use axon_agent_runtime::{
    AgentModelMessage, AgentPath, ProviderToolCall,
    SUBAGENT_FORK_CONTEXT_REASON_CHILD_USAGE_HINT_ADDED,
};
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
        RunSubagentExecutionStatus, RunSubagentExecutor,
    },
};

fn provider_tool_call_with_args(name: &str, arguments: &serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new(name, name, arguments.to_string())
}

fn fixture_request(current_agent_path: AgentPath) -> RunRequest {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 配置路径决定运行态根：必须用测试隔离目录，否则 data/runtime.sqlite3 与 data/logs/tee 会写进 crate 目录。
    let config_path = axon_test_support::test_temp_path("config.toml");
    let session_id = super::unique_test_thread_session_id("session-subagent-path");
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
        prompt: "子代理路径语义".to_owned(),
        quoted_message: None,
        structured_output: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: axon_agent_runtime::AgentModelPayload {
            messages: vec![AgentModelMessage::text("user", "子代理路径语义")],
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
        current_agent_path,
        agent_control: Arc::new(Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: Arc::new(RunningSubagentExecutor) as Arc<dyn RunSubagentExecutor>,
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        steer_inbox: None,
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

#[test]
fn child_subagent_tools_resolve_relative_paths_from_current_agent() {
    let mut request = fixture_request(AgentPath::parse("/root/review").unwrap());
    request.subagent_executor = Arc::new(CaptureSubagentExecutor {
        captured: Arc::new(Mutex::new(None)),
    });
    {
        let mut control = request.agent_control.lock().unwrap();
        control
            .spawn(axon_agent_runtime::AgentSpawnRequest {
                parent_path: AgentPath::root(),
                task_name: "review".to_owned(),
                message: "父级审查".to_owned(),
                nickname: None,
                role: None,
            })
            .expect("应可预置当前子代理");
    }

    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "security",
            "task": "审查安全风险"
        }),
    );
    let spawn_receipts = super::execute_supported_tool_calls(&request, &[spawn], None);

    assert_eq!(spawn_receipts.len(), 1);
    assert_eq!(spawn_receipts[0].status, "completed");

    let message = provider_tool_call_with_args(
        "builtin.send_message",
        &serde_json::json!({
            "target": "security",
            "message": "补充检查依赖风险",
            "intent": "让安全子代理补查依赖"
        }),
    );
    let message_receipts = super::execute_supported_tool_calls(&request, &[message], None);

    assert_eq!(message_receipts.len(), 1);
    assert_eq!(message_receipts[0].status, "completed");
}

#[test]
fn spawn_fork_context_injects_child_usage_hint_for_real_child_request() {
    let mut request = fixture_request(AgentPath::root());
    let captured = Arc::new(Mutex::new(None));
    request.subagent_executor = Arc::new(CaptureSubagentExecutor {
        captured: captured.clone(),
    });
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动",
            "fork_turns": "all"
        }),
    );
    let spawn_receipts = super::execute_supported_tool_calls(&request, &[spawn], None);

    assert_eq!(spawn_receipts.len(), 1);
    assert_eq!(spawn_receipts[0].status, "completed");
    assert_eq!(
        spawn_receipts[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/payload/forkContext/reasons"))
            .and_then(serde_json::Value::as_array)
            .map(|reasons| reasons.iter().any(|reason| {
                reason.as_str() == Some(SUBAGENT_FORK_CONTEXT_REASON_CHILD_USAGE_HINT_ADDED)
            })),
        Some(true)
    );

    let input = captured
        .lock()
        .unwrap()
        .clone()
        .expect("spawn 应启动子代理执行输入");
    assert_eq!(input.task_path.as_str(), "/root/review");
    assert!(input.fork_context.messages.iter().any(|message| {
        message.role == "developer"
            && message
                .content
                .contains("当前 canonical task path: /root/review")
            && message.content.contains("直接父代理路径: /root")
    }));
}

struct CaptureSubagentExecutor {
    captured: Arc<Mutex<Option<RunSubagentExecutionInput>>>,
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

impl RunSubagentExecutor for CaptureSubagentExecutor {
    fn execute(&self, _input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        Some(RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: "captured".to_owned(),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        })
    }

    fn start(
        &self,
        input: RunSubagentExecutionInput,
        completion: RunSubagentCompletionSink,
    ) -> bool {
        *self.captured.lock().unwrap() = Some(input.clone());
        let output = self.execute(input.clone()).expect("捕获执行器应返回输出");
        completion.complete(&input, &output).unwrap();
        true
    }
}
