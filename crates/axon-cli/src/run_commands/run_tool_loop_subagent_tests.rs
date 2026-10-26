use axon_agent_runtime::ProviderToolCall;
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{
    ExecutionRequestId, ModelProfileId, PermissionSnapshotId, ToolCallId, ToolExecutionPermission,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    run_commands::{
        RunRequest,
        run_subagent_executor::{
            NoopRunSubagentExecutor, RunSubagentCompletionSink, RunSubagentExecutionInput,
            RunSubagentExecutionOutput, RunSubagentExecutionStatus, RunSubagentExecutor,
        },
    },
    tool_orchestrator::{ToolExecutionRequest, ToolKind},
};

fn provider_tool_call(name: &str) -> ProviderToolCall {
    ProviderToolCall::new(name, name, "{}")
}

fn provider_tool_call_with_args(name: &str, arguments: &serde_json::Value) -> ProviderToolCall {
    ProviderToolCall::new(name, name, arguments.to_string())
}

fn fixture_request() -> RunRequest {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // 配置路径决定运行态根：必须用测试隔离目录，否则 data/runtime.sqlite3 与 data/logs/tee 会写进 crate 目录。
    let config_path = axon_test_support::test_temp_path("config.toml");
    let session_id = super::unique_test_thread_session_id("session-subagent-control");
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
        structured_output: None,
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
        agent_control: Arc::new(Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: Arc::new(RunningSubagentExecutor),
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        steer_inbox: None,
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

#[test]
fn subagent_spawn_calls_can_run_in_parallel_but_mixed_control_calls_stay_serial() {
    let spawn_requests = vec![
        ToolExecutionRequest {
            tool_call: provider_tool_call("builtin.spawn_agent"),
            tool_call_id: ToolCallId(1),
            part_id: "tool-part:test:subagent:1".to_owned(),
            execution_request_id: ExecutionRequestId(1),
            kind: ToolKind::Subagent,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
            execution_permission: ToolExecutionPermission::Allowed,
        },
        ToolExecutionRequest {
            tool_call: provider_tool_call("builtin.spawn_agent"),
            tool_call_id: ToolCallId(2),
            part_id: "tool-part:test:subagent:2".to_owned(),
            execution_request_id: ExecutionRequestId(2),
            kind: ToolKind::Subagent,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
            execution_permission: ToolExecutionPermission::Allowed,
        },
    ];
    let mixed_requests = vec![
        spawn_requests[0].clone(),
        ToolExecutionRequest {
            tool_call: provider_tool_call("builtin.list_agents"),
            tool_call_id: ToolCallId(3),
            part_id: "tool-part:test:subagent:3".to_owned(),
            execution_request_id: ExecutionRequestId(3),
            kind: ToolKind::Subagent,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            execution_permission: ToolExecutionPermission::Allowed,
        },
    ];
    let mut duplicate_requests = spawn_requests.clone();
    duplicate_requests[0].tool_call = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动",
            "fork_turns": "all"
        }),
    );
    duplicate_requests[1].tool_call = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review_2",
            "task": "  审查当前改动  ",
            "fork_turns": "all"
        }),
    );

    assert!(super::tool_calls_can_run_in_parallel(&spawn_requests));
    assert!(!super::tool_calls_can_run_in_parallel(&duplicate_requests));
    assert!(!super::tool_calls_can_run_in_parallel(&mixed_requests));
}

#[test]
fn distinct_fast_delegates_are_parallel_safe_under_default_scheduling() {
    let mut fast = spawn_requests_for_test("builtin.delegate_to_fast", 1, "扫描 auth");
    fast.extend(spawn_requests_for_test(
        "builtin.delegate_to_fast",
        2,
        "检查配置",
    ));

    assert!(super::tool_calls_can_run_in_parallel(&fast));
}

fn spawn_requests_for_test(
    tool_name: &str,
    tool_call_id: u64,
    task: &str,
) -> Vec<ToolExecutionRequest> {
    vec![ToolExecutionRequest {
        tool_call: provider_tool_call_with_args(tool_name, &serde_json::json!({"task": task})),
        tool_call_id: ToolCallId(tool_call_id),
        part_id: format!("tool-part:test:{tool_call_id}"),
        execution_request_id: ExecutionRequestId(tool_call_id),
        kind: ToolKind::Subagent,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        execution_permission: ToolExecutionPermission::Allowed,
    }]
}

#[test]
fn duplicate_spawn_intents_in_same_parent_turn_reuse_first_session() {
    let request = fixture_request();
    let calls = [
        provider_tool_call_with_args(
            "builtin.spawn_agent",
            &serde_json::json!({
                "task_name": "review",
                "task": "审查当前改动",
                "fork_turns": "all"
            }),
        ),
        provider_tool_call_with_args(
            "builtin.spawn_agent",
            &serde_json::json!({
                "task_name": "review_2",
                "task": "  审查当前改动  ",
                "fork_turns": "all"
            }),
        ),
    ];

    let receipts = super::execute_supported_tool_calls(&request, &calls, None);

    assert_eq!(receipts.len(), 2);
    assert!(receipts.iter().all(|receipt| receipt.status == "completed"));
    assert!(receipts.iter().all(|receipt| {
        receipt
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/payload/session/taskName"))
            .and_then(serde_json::Value::as_str)
            == Some("/root/review")
    }));
    assert_eq!(
        request.subagent_sessions.lock().unwrap().sessions().len(),
        1
    );
}

#[test]
fn subagent_control_persists_across_tool_batches_in_same_run() {
    let request = fixture_request();
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    let spawn_receipts = super::execute_supported_tool_calls(&request, &[spawn], None);

    assert_eq!(spawn_receipts.len(), 1);
    assert_eq!(spawn_receipts[0].status, "completed");
    let session = spawn_receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/session"))
        .expect("spawn 回执必须包含子代理运行会话元数据");
    assert_eq!(
        session.get("taskName").and_then(serde_json::Value::as_str),
        Some("/root/review")
    );
    assert_eq!(
        session.get("status").and_then(serde_json::Value::as_str),
        Some("running")
    );
    assert_eq!(
        session
            .get("modelProfileId")
            .and_then(serde_json::Value::as_u64),
        Some(41)
    );
    assert_eq!(
        session
            .get("permissionSnapshotId")
            .and_then(serde_json::Value::as_u64),
        Some(42)
    );
    assert!(
        session
            .get("executionId")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|execution_id| execution_id > 0)
    );
    assert_eq!(
        request
            .subagent_sessions
            .lock()
            .unwrap()
            .sessions()
            .first()
            .map(|handle| (
                handle.summary.as_str(),
                handle.model_profile_id,
                handle.permission_snapshot_id,
            )),
        Some(("审查当前改动", ModelProfileId(41), PermissionSnapshotId(42)))
    );

    let list = provider_tool_call_with_args("builtin.list_agents", &serde_json::json!({}));
    let list_receipts = super::execute_supported_tool_calls(&request, &[list], None);

    assert_eq!(list_receipts.len(), 1);
    assert_eq!(list_receipts[0].status, "completed");
    assert!(list_receipts[0].summary.contains("/root/review"));
}

#[test]
fn spawn_with_unavailable_executor_is_blocked_terminal_state() {
    let mut request = fixture_request();
    request.subagent_executor = Arc::new(NoopRunSubagentExecutor);
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    let spawn_receipts = super::execute_supported_tool_calls(&request, &[spawn], None);

    assert_eq!(spawn_receipts.len(), 1);
    assert_eq!(spawn_receipts[0].status, "blocked");
    assert_eq!(
        request
            .agent_control
            .lock()
            .unwrap()
            .active_execution_count(),
        0
    );
    assert_eq!(
        request
            .agent_control
            .lock()
            .unwrap()
            .status(&axon_agent_runtime::AgentPath::parse("/root/review").unwrap()),
        axon_agent_runtime::AgentStatus::Errored("subagent_executor:not_started".to_owned())
    );
    assert_eq!(
        request
            .subagent_sessions
            .lock()
            .unwrap()
            .session_by_task_path("/root/review")
            .map(|handle| (handle.status, handle.summary.as_str())),
        Some((
            axon_agent_runtime::SubagentSessionStatus::Errored,
            "subagent_executor:not_started"
        ))
    );
}

#[test]
fn subagent_send_message_stays_queue_only_without_triggering_executor() {
    let mut request = fixture_request();
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    super::execute_supported_tool_calls(&request, &[spawn], None);
    request.subagent_executor = Arc::new(FixtureSubagentExecutor);

    let message = provider_tool_call_with_args(
        "builtin.send_message",
        &serde_json::json!({
            "target": "/root/review",
            "message": "只入队，不触发执行",
            "intent": "确认只入队不触发执行"
        }),
    );
    let message_receipts = super::execute_supported_tool_calls(&request, &[message], None);

    assert_eq!(
        message_receipts[0]
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.pointer("/payload/turnRequested"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        request
            .agent_control
            .lock()
            .unwrap()
            .status(&axon_agent_runtime::AgentPath::parse("/root/review").unwrap()),
        axon_agent_runtime::AgentStatus::Running
    );
    assert_eq!(
        request
            .subagent_sessions
            .lock()
            .unwrap()
            .session_by_task_path("/root/review")
            .map(|handle| handle.status),
        Some(axon_agent_runtime::SubagentSessionStatus::Running)
    );
}

#[test]
fn subagent_completion_is_visible_through_the_job_channel() {
    let mut request = fixture_request();
    request.subagent_executor = Arc::new(FixtureSubagentExecutor);
    let spawn = provider_tool_call_with_args(
        "builtin.spawn_agent",
        &serde_json::json!({
            "task_name": "review",
            "task": "审查当前改动"
        }),
    );
    let spawn_receipts = super::execute_supported_tool_calls(&request, &[spawn], None);
    let job_id = spawn_receipts[0]
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.pointer("/payload/jobId"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| value.parse::<u64>().ok())
        .expect("spawn 回执必须携带 jobId");

    // 完成结果不再靠阻塞等待拉取：模型按 job_id 用 job_output 收集。
    let output = axon_agent_runtime::JobRuntime::global()
        .read(
            axon_agent_runtime::JobId(job_id),
            &request.session_id,
            Some(0),
        )
        .expect("子代理作业必须可按 job_id 读取");
    assert_eq!(output.status, axon_agent_runtime::JobStatus::Completed);
    assert!(
        output
            .stdout
            .preview
            .contains("/root/review => 审查当前改动"),
        "回执必须携带子代理结果正文：{}",
        output.stdout.preview
    );

    // 会话元数据同步为终态：等待与终止都不再占用 active turn。
    assert_eq!(
        request
            .subagent_sessions
            .lock()
            .unwrap()
            .session_by_task_path("/root/review")
            .map(|handle| handle.status),
        Some(axon_agent_runtime::SubagentSessionStatus::Completed)
    );
}

/// 保持运行的后台执行器：不写回完成，用于观察运行中状态。
#[derive(Debug)]
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

/// 立即完成的执行器：把结果写回 completion，用于观察终态回执。
#[derive(Debug)]
struct FixtureSubagentExecutor;

impl RunSubagentExecutor for FixtureSubagentExecutor {
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        Some(RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: format!(
                "{} => {} [{} retained]",
                input.task_path, input.message, input.fork_context.retained_count
            ),
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        })
    }
}
