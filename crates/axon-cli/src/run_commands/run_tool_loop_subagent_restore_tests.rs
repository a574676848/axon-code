use axon_agent_runtime::{
    AgentControlConfig, AgentControlRuntime, ProviderToolCall, agent_run_assistant_part_id,
};
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{ModelProfileId, PermissionSnapshotId};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    OutputFormat, RunCommand,
    run_commands::{
        RunAgentIdentity, RunRequest, RunRequestBuildOptions, RunRequestSession, RunSessionRecord,
        run_subagent_executor::{
            RunSubagentCompletionSink, RunSubagentExecutionInput, RunSubagentExecutionOutput,
            RunSubagentExecutor,
        },
        run_thread_store::PreparedRunSession,
    },
};

fn provider_tool_call_with_args(name: &str, arguments: &serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new(name, name, arguments.to_string())
}

fn run_command(prompt: &str) -> RunCommand {
    RunCommand {
        prompt: prompt.to_owned(),
        model: None,
        reasoning_effort: None,
        format: Some(OutputFormat::Json),
        continue_last: false,
        session: None,
        file: Vec::new(),
        timeout: None,
        tool: Vec::new(),
    }
}

fn config_with_fixture_model() -> axon_config::AxonConfig {
    let mut config = axon_config::AxonConfig::default();
    config.providers.push(axon_config::ProviderConfig::new(
        "fixture",
        "openai-chatcompletions",
        "https://provider.invalid/v1",
    ));
    config.models.push(axon_config::ModelConfig {
        name: "fixture-chat".to_owned(),
        id: "fixture-chat".to_owned(),
        provider: "fixture".to_owned(),
        context_window: 32_000,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: false,
        embedding_dimension: 0,
    });
    config.agent.default_model = "fixture-chat".to_owned();
    config.agent.fast_model = "fixture-chat".to_owned();
    config
}

fn previous_session_record(session_id: &str) -> RunSessionRecord {
    RunSessionRecord {
        session_id: session_id.to_owned(),
        parent_session_id: None,
        created_at_ms: 1,
        startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
        cwd: None,
        provider: "fixture".to_owned(),
        model: "fixture-chat".to_owned(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        prompt: "首轮任务".to_owned(),
        local_images: Vec::new(),
        local_files: Vec::new(),
        response: "已创建子代理".to_owned(),
        assistant_part_id: agent_run_assistant_part_id(1),
        usage: None,
        tool_receipts: Vec::new(),
        tool_calls: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        failure: None,
        upstream_model: None,
        reasoning_effort: None,
        request_path: None,
        speed_mode: None,
        ttft_ms: None,
        generation_speed_tps: None,
        injected_context_messages: Vec::new(),
    }
}

fn prepared_session(session_id: &str) -> PreparedRunSession {
    let record = previous_session_record(session_id);
    PreparedRunSession {
        record,
        model_history_json: "[]".to_owned(),
    }
}

fn fixture_request() -> RunRequest {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 配置路径决定运行态根：必须用测试隔离目录，否则 data/runtime.sqlite3 与 data/logs/tee 会写进 crate 目录。
    let config_path = axon_test_support::test_temp_path("config.toml");
    let session_id = super::unique_test_thread_session_id("session-subagent-restore");
    let (startup, execution_tools) =
        crate::run_commands::run_startup_snapshot::test_startup_runtime(
            &config_path,
            &cwd,
            &session_id,
        );
    RunRequest {
        user_question_port: None,
        client_channel: axon_client_identity::ClientChannel::Cli,
        config_path,
        session_id: session_id.clone(),
        invocation_id: format!("test-invocation:{session_id}"),
        resumed_from: None,
        parent_session_id: None,
        cwd,
        prompt: "恢复子代理".to_owned(),
        quoted_message: None,
        structured_output: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: axon_agent_runtime::AgentModelPayload {
            messages: vec![axon_agent_runtime::AgentModelMessage::text(
                "user",
                "恢复子代理",
            )],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        },
        startup,
        execution_tools,
        provider: ProviderConfig::default(),
        model: ModelConfig::default(),
        reasoning_effort: None,
        channel_chain: Vec::new(),
        routing_key: "test".to_owned(),
        balancer: ChannelBalancerConfig::default(),
        timeout: Duration::from_secs(20),
        memory_recall: None,
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        model_profile_id: ModelProfileId(41),
        permission_snapshot_id: PermissionSnapshotId(42),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: Arc::new(Mutex::new(AgentControlRuntime::default())),
        agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: Arc::new(RunningSubagentExecutor),
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        steer_inbox: None,
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

#[test]
fn new_run_request_reopens_file_graph_and_restores_child() {
    let workspace = axon_test_support::test_temp_root().join(format!(
        "axon-subagent-request-restore-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos()
    ));
    std::fs::create_dir_all(&workspace).unwrap();
    let config_path = workspace.join("axon.toml");
    std::fs::write(&config_path, "").unwrap();
    let config = config_with_fixture_model();
    let startup =
        Arc::new(crate::run_commands::RunStartupSnapshot::build(&config_path, &workspace).unwrap());

    let first = RunRequest::from_command(
        &run_command("首轮任务"),
        &config,
        &config_path,
        RunRequestSession::with_id(None, crate::run_commands::new_run_session_id()),
        Some(&workspace),
        RunRequestBuildOptions {
            client_channel: axon_client_identity::ClientChannel::Cli,
            startup: Arc::clone(&startup),
            cancel_token: None,
            subagent_event_bus: None,
            subagent_session_sinks: None,
            agent_identity: RunAgentIdentity::PrimaryPrivate,
            performance: None,
            workspace_snapshot_prewarm: None,
        },
    )
    .expect("首轮 RunRequest 应可构造");
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    let spawn_receipts = super::execute_supported_tool_calls(&first, &[spawn], None);
    assert_eq!(spawn_receipts[0].status, "completed");
    assert!(workspace.join("data").join("subagent-graph.json").exists());

    let previous = prepared_session(&first.session_id);
    let second = RunRequest::from_command(
        &run_command("恢复后继续"),
        &config,
        &config_path,
        RunRequestSession::with_id(Some(&previous), previous.record.session_id.clone()),
        Some(&workspace),
        RunRequestBuildOptions {
            client_channel: axon_client_identity::ClientChannel::Cli,
            startup,
            cancel_token: None,
            subagent_event_bus: None,
            subagent_session_sinks: None,
            agent_identity: RunAgentIdentity::PrimaryPrivate,
            performance: None,
            workspace_snapshot_prewarm: None,
        },
    )
    .expect("第二轮 RunRequest 应可构造");
    assert_eq!(
        second
            .agent_control
            .lock()
            .unwrap()
            .status(&axon_agent_runtime::AgentPath::parse("/root/review").unwrap()),
        axon_agent_runtime::AgentStatus::NotFound
    );

    let list = provider_tool_call_with_args("builtin.list_agents", &serde_json::json!({}));
    let receipts = super::execute_supported_tool_calls(&second, &[list], None);

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].status, "completed");
    let agents = receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/agents"))
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert!(
        agents
            .iter()
            .any(|agent| agent["agentName"] == "/root/review"),
        "新 run 必须重新打开 graph store 并恢复子代理可见性"
    );
    let _ = std::fs::remove_dir_all(&workspace);
}

#[test]
fn list_agents_restores_open_tree_visibility_after_control_reset() {
    let request = fixture_request();
    for task_name in ["review", "build"] {
        let spawn = provider_tool_call_with_args(
            "builtin.spawn_agent",
            &serde_json::json!({
                "task_name": task_name,
                "task": format!("处理 {task_name}")
            }),
        );
        super::execute_supported_tool_calls(&request, &[spawn], None);
    }
    *request.agent_control.lock().unwrap() = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 16,
        max_active_turns: 16,
        max_depth: 1,
    });

    let list = provider_tool_call_with_args("builtin.list_agents", &serde_json::json!({}));
    let receipts = super::execute_supported_tool_calls(&request, &[list], None);

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].status, "completed");
    let agents = receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/agents"))
        .and_then(serde_json::Value::as_array)
        .unwrap();
    let agent_names = agents
        .iter()
        .filter_map(|agent| agent["agentName"].as_str())
        .collect::<Vec<_>>();
    assert!(agent_names.contains(&"/root/review"));
    assert!(
        !agent_names.contains(&"/root/review/security"),
        "恢复必须使用当前 max_depth，而不是默认值"
    );
}

#[test]
fn restore_uses_current_root_thread_id_instead_of_fixed_zero() {
    let request = fixture_request();
    *request.agent_control.lock().unwrap() =
        AgentControlRuntime::new_with_root_thread_id(AgentControlConfig::default(), 41);
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    super::execute_supported_tool_calls(&request, &[spawn], None);
    *request.agent_control.lock().unwrap() =
        AgentControlRuntime::new_with_root_thread_id(AgentControlConfig::default(), 41);

    let list = provider_tool_call_with_args("builtin.list_agents", &serde_json::json!({}));
    let receipts = super::execute_supported_tool_calls(&request, &[list], None);

    assert_eq!(receipts[0].status, "completed");
    let agents = receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/agents"))
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert!(
        agents
            .iter()
            .any(|agent| agent["agentName"] == "/root/review")
    );
}

#[test]
fn restore_uses_current_agent_control_limits_instead_of_defaults() {
    let mut request = fixture_request();
    let spawn_review = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({"task_name": "review", "task": "审查当前改动"}),
    );
    super::execute_supported_tool_calls(&request, &[spawn_review], None);
    request.current_agent_path = axon_agent_runtime::AgentPath::parse("/root/review").unwrap();
    let spawn_security = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({"task_name": "security", "task": "审查安全边界"}),
    );
    let nested_receipts = super::execute_supported_tool_calls(&request, &[spawn_security], None);
    assert_eq!(nested_receipts[0].status, "completed");
    request.current_agent_path = axon_agent_runtime::AgentPath::root();
    *request.agent_control.lock().unwrap() = AgentControlRuntime::new(AgentControlConfig {
        max_agents: 16,
        max_active_turns: 16,
        max_depth: 1,
    });

    let list = provider_tool_call_with_args("builtin.list_agents", &serde_json::json!({}));
    let receipts = super::execute_supported_tool_calls(&request, &[list], None);

    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].status, "completed");
    let agents = receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/agents"))
        .and_then(serde_json::Value::as_array)
        .unwrap();
    let agent_names = agents
        .iter()
        .filter_map(|agent| agent["agentName"].as_str())
        .collect::<Vec<_>>();
    assert!(agent_names.contains(&"/root/review"));
    assert!(
        !agent_names.contains(&"/root/review/security"),
        "恢复必须使用当前 max_depth，而不是默认值"
    );
}

struct RunningSubagentExecutor;

impl RunSubagentExecutor for RunningSubagentExecutor {
    fn execute(&self, _input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        None
    }

    fn start(
        &self,
        _input: RunSubagentExecutionInput,
        _completion: RunSubagentCompletionSink,
    ) -> bool {
        true
    }
}
