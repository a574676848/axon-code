use axon_agent_runtime::{AgentControlConfig, AgentControlRuntime, ProviderToolCall};
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{ModelProfileId, PermissionSnapshotId};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::run_commands::{
    RunRequest,
    run_subagent_executor::{
        RunSubagentCompletionSink, RunSubagentExecutionInput, RunSubagentExecutionOutput,
        RunSubagentExecutor,
    },
};

fn provider_tool_call_with_args(name: &str, arguments: &serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new(name, name, arguments.to_string())
}

fn fixture_request() -> RunRequest {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 配置路径决定运行态根：必须用测试隔离目录，否则 data/runtime.sqlite3 与 data/logs/tee 会写进 crate 目录。
    let config_path = axon_test_support::test_temp_path("config.toml");
    let session_id = super::unique_test_thread_session_id("session-subagent-graph");
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
        prompt: "拆分子任务".to_owned(),
        quoted_message: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: axon_agent_runtime::AgentModelPayload {
            messages: vec![axon_agent_runtime::AgentModelMessage::text(
                "user",
                "拆分子任务",
            )],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        },
        persona_warnings: Vec::new(),
        startup,
        execution_tools,
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
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
        auto_compaction_threshold: 0.8,
        performance: None,
        structured_output: None,
    }
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

#[test]
fn subagent_tool_graph_store_restores_only_open_children() {
    let request = fixture_request();
    let mut closed_job_id = None;
    for task_name in ["review", "build"] {
        let spawn = provider_tool_call_with_args(
            "builtin.spawn_agent",
            &serde_json::json!({
                "task_name": task_name,
                "task": format!("处理 {task_name}")
            }),
        );
        let receipts = super::execute_supported_tool_calls(&request, &[spawn], None);
        assert_eq!(receipts[0].status, "completed");
        if task_name == "review" {
            closed_job_id = receipts[0]
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.pointer("/payload/jobId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
        }
    }

    // 终止不再由子代理工具承担：`job_kill` 走 job_id 通道，并在同一路径上
    // 把关闭同步进 graph store。
    let close = provider_tool_call_with_args(
        "builtin.job_kill",
        &serde_json::json!({ "job_id": closed_job_id.expect("review 必须回带 jobId") }),
    );
    let close_receipts = super::execute_supported_tool_calls(&request, &[close], None);
    assert_eq!(close_receipts[0].status, "killed");

    let restored = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(AgentControlRuntime::restore_open_tree_from_store(
            request.agent_graph_store.as_ref(),
            0,
            AgentControlConfig::default(),
        ))
        .unwrap()
        .0;
    let agents = restored
        .list_agents(&axon_agent_runtime::AgentPath::root(), None)
        .unwrap()
        .into_iter()
        .map(|agent| agent.agent_name)
        .collect::<Vec<_>>();

    assert_eq!(agents, ["/root", "/root/build"]);
}

#[test]
fn subagent_control_isolated_between_run_requests() {
    let first = fixture_request();
    let second = fixture_request();
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    let _ = super::execute_supported_tool_calls(&first, &[spawn], None);

    let list = provider_tool_call_with_args("builtin.list_agents", &serde_json::json!({}));
    let second_receipts = super::execute_supported_tool_calls(&second, &[list], None);

    assert_eq!(second_receipts.len(), 1);
    assert_eq!(second_receipts[0].status, "completed");
    assert!(!second_receipts[0].summary.contains("/root/review"));
}
