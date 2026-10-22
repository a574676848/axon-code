use super::*;

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, mpsc},
    time::Duration,
};

use axon_agent_runtime::{
    LocalAgentGraphStore, SubagentForkContextInput, SubagentForkContextReport,
    SubagentForkContextRuntime, SubagentForkTurns,
};
use axon_domain::{ModelProfileId, SessionId};
use axon_protocol::wire::{
    WireEventKind, WireLifecycleStatus, WireProviderStatus, WireProviderStatusPhase,
    WireProviderTransport,
};
use axon_session::{JsonlThreadEventStore, ThreadService, ThreadStatus, TurnStatus};

use crate::run_commands::{
    run_execution::{RunExecutionError, RunResponse},
    run_progress::RunProgressSink,
};

const FIXTURE_SERVER_TIMEOUT: Duration = Duration::from_millis(900);
const FIXTURE_ASYNC_COMPLETION_TIMEOUT: Duration = Duration::from_secs(5);
const FIXTURE_PROVIDER_BASE_URL: &str = "http://provider.fixture.invalid/v1";
const MOCK_RESPONSE_CONTENT: &str = "子代理完成";
const MOCK_ASSISTANT_PART_ID: &str = "assistant-part:subagent-fixture";
const MOCK_ASSISTANT_MESSAGE_ID: &str = "assistant-message:subagent-fixture";
const MOCK_INPUT_TOKENS: u64 = 7;
const MOCK_OUTPUT_TOKENS: u64 = 5;
const MOCK_TOTAL_TOKENS: u64 = MOCK_INPUT_TOKENS + MOCK_OUTPUT_TOKENS;
const MOCK_ATTEMPT_INDEX: u64 = 1;
const MOCK_HTTP_STATUS: u16 = 200;

fn temp_config_path(label: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root()
        .join(format!(
            "axon-subagent-executor-{label}-{}-{nonce}",
            std::process::id()
        ))
        .join("config.toml")
}

fn fixture_environment(config_path: PathBuf) -> ProviderRunSubagentEnvironment {
    let provider = ProviderConfig::new(
        "fixture",
        axon_config::PROVIDER_PROTOCOL_OPENAI,
        FIXTURE_PROVIDER_BASE_URL,
    );
    let model = ModelConfig {
        name: "fixture-chat".to_owned(),
        id: "fixture-chat".to_owned(),
        provider: "fixture".to_owned(),
        context_window: 32_000,
        ..ModelConfig::default()
    };
    let mut config = axon_config::AxonConfig::default();
    config.providers.push(provider.clone());
    config.models.push(model.clone());
    config.agent.default_model = "fixture-chat".to_owned();
    config.agent.fast_model = "fixture-chat".to_owned();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let startup =
        Arc::new(crate::run_commands::RunStartupSnapshot::build(&config_path, &cwd).unwrap());
    ProviderRunSubagentEnvironment {
        config,
        config_path,
        parent_session_id: "parent-session".to_owned(),
        cwd,
        provider: provider.clone(),
        model: model.clone(),
        reasoning_effort: Some("high".to_owned()),
        channel_chain: vec![(model, provider)],
        routing_key: "fixture-chat".to_owned(),
        balancer: ChannelBalancerConfig::default(),
        timeout: Duration::from_secs(10),
        permission_snapshot_id: PermissionSnapshotId(9),
        startup,
        cancel_token: None,
        agent_control: Arc::new(Mutex::new(AgentControlRuntime::default())),
        agent_graph_store: Arc::new(LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(SubagentSessionRuntime::default())),
        subagent_event_bus: None,
        subagent_session_sinks: None,
        subagent_cancel_tokens: Arc::new(Mutex::new(HashMap::new())),
        workspace_files: Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
    }
}

fn mock_provider_turn(
    request: &RunRequest,
    progress: Option<&RunProgressSink<'_>>,
) -> Result<RunResponse, Box<RunExecutionError>> {
    assert_eq!(request.prompt, "审查当前改动");
    let progress = progress.expect("子代理进程内 provider runner 应接收进度投影器");
    let provider_status = |phase| WireProviderStatus {
        provider: request.provider.name.clone(),
        model: request.model.name.clone(),
        phase,
        transport: WireProviderTransport::Http,
        attempt_index: MOCK_ATTEMPT_INDEX,
        http_status: None,
        error_message: None,
        next_provider: None,
        next_model: None,
    };
    progress.provider_status(provider_status(WireProviderStatusPhase::Connecting));
    progress.provider_status(provider_status(WireProviderStatusPhase::Connected));
    progress.assistant_delta(
        MOCK_ASSISTANT_PART_ID,
        MOCK_ASSISTANT_MESSAGE_ID,
        MOCK_RESPONSE_CONTENT,
    );
    let token_usage = TokenUsageTelemetry::new(MOCK_INPUT_TOKENS, 0, 0, MOCK_OUTPUT_TOKENS, 0);
    progress.token_usage_updated(token_usage);

    Ok(RunResponse {
        content: MOCK_RESPONSE_CONTENT.to_owned(),
        assistant_part_id: MOCK_ASSISTANT_PART_ID.to_owned(),
        http_status: MOCK_HTTP_STATUS,
        token_usage: Some(token_usage),
        tool_receipts: Vec::new(),
        tool_calls: Vec::new(),
        provider: request.provider.name.clone(),
        model: request.model.name.clone(),
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        max_tokens_truncated: false,
    })
}

/// 有界等待 continuation 落盘。
///
/// 终态 lifecycle 事件与 continuation 落盘是完成路径上的两步：异步执行器先把权威
/// 终态投影到 wire，再写持久引用。测试按上限等待而不是假设两者瞬时一致，否则会把
/// 「顺序」误判成「缺失」。
fn await_pending_continuation(
    store: &axon_session::SqliteRuntimeStore,
    session_id: &str,
) -> Vec<axon_session::AgentContinuationRecord> {
    let deadline = std::time::Instant::now() + FIXTURE_ASYNC_COMPLETION_TIMEOUT;
    loop {
        let pending = axon_session::AgentContinuationStore::pending_continuations(
            store, session_id, session_id, 1_000, 10,
        )
        .expect("读取待装配 continuation");
        if !pending.is_empty() || std::time::Instant::now() >= deadline {
            return pending;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fixture_execution_input(task_path: &str) -> RunSubagentExecutionInput {
    RunSubagentExecutionInput {
        task_path: AgentPath::parse(task_path).unwrap(),
        message: "验证工具权限继承".to_owned(),
        fork_context: SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms: 1,
        max_runtime_ms: None,
        model_intent: RunSubagentModelIntent::Inherit,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
    }
}

#[test]
fn memory_write_count_only_accepts_completed_receipts_with_persisted_ids() {
    let receipt = |tool_name: &str, status: &str, memory_id: serde_json::Value| {
        crate::run_commands::run_tool_loop::RunToolReceipt {
            tool_call_id: 0,
            part_id: "tool-part:memory-write".to_owned(),
            provider_call_id: format!("{tool_name}:{status}"),
            tool_name: tool_name.to_owned(),
            status: status.to_owned(),
            summary: "memory receipt".to_owned(),
            metadata: Some(serde_json::json!({ "memoryId": memory_id })),
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: None,
        }
    };
    let response = crate::run_commands::run_execution::RunResponse {
        content: "done".to_owned(),
        assistant_part_id: "assistant-part:memory-write".to_owned(),
        http_status: 200,
        token_usage: None,
        tool_receipts: vec![
            receipt("builtin.memory_write", "completed", serde_json::json!(7)),
            receipt("builtin.memory_write", "completed", serde_json::Value::Null),
            receipt("builtin.memory_write", "failed", serde_json::json!(8)),
            receipt("builtin.search", "completed", serde_json::json!(9)),
        ],
        tool_calls: Vec::new(),
        provider: "fixture".to_owned(),
        model: "fixture-chat".to_owned(),
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        max_tokens_truncated: false,
    };

    assert_eq!(persisted_memory_write_count(&response), 1);
}

#[test]
fn empty_result_child_turn_is_marked_errored() {
    fn empty_provider_turn(
        request: &RunRequest,
        _progress: Option<&RunProgressSink<'_>>,
    ) -> Result<RunResponse, Box<RunExecutionError>> {
        Ok(RunResponse {
            content: String::new(),
            assistant_part_id: MOCK_ASSISTANT_PART_ID.to_owned(),
            http_status: MOCK_HTTP_STATUS,
            token_usage: None,
            tool_receipts: Vec::new(),
            tool_calls: Vec::new(),
            provider: request.provider.name.clone(),
            model: request.model.name.clone(),
            channel_attempts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
            max_tokens_truncated: false,
        })
    }

    let config_path = temp_config_path("empty-result");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        config_path.parent().unwrap().join("AGENTS.md"),
        "子代理空结果规则",
    )
    .unwrap();
    let executor = ProviderRunSubagentExecutor::with_provider_turn_runner(
        fixture_environment(config_path.clone()),
        empty_provider_turn,
    );
    let input = fixture_execution_input("/root/review");
    let output = executor
        .execute(input)
        .expect("provider executor 应返回子代理执行结果");

    assert_eq!(output.status, RunSubagentExecutionStatus::Errored);
    assert!(
        output.summary.contains("未产生任何结果"),
        "空结果摘要应说明未产生任何结果: {}",
        output.summary
    );
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[path = "run_subagent_executor_request_tests.rs"]
mod request_tests;

#[test]
fn provider_executor_runs_child_provider_turn() {
    let config_path = temp_config_path("provider-turn");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        config_path.parent().unwrap().join("AGENTS.md"),
        "子代理 provider 规则",
    )
    .unwrap();

    let executor = ProviderRunSubagentExecutor::with_provider_turn_runner(
        fixture_environment(config_path.clone()),
        mock_provider_turn,
    );
    let task_path = AgentPath::parse("/root/review").unwrap();
    let output = executor
        .execute(RunSubagentExecutionInput {
            task_path: task_path.clone(),
            message: "审查当前改动".to_owned(),
            fork_context: SubagentForkContextReport {
                messages: Vec::new(),
                original_count: 0,
                retained_count: 0,
                dropped_count: 0,
                reasons: Vec::new(),
            },
            started_at_ms: 1,
            max_runtime_ms: None,
            model_intent: RunSubagentModelIntent::Inherit,
            structured_output: None,
            system_prompt_override: None,
            tools_enabled: true,
        })
        .expect("provider executor 应返回子代理执行结果");

    assert_eq!(output.status, RunSubagentExecutionStatus::Completed);
    assert_eq!(output.summary, MOCK_RESPONSE_CONTENT);
    assert_eq!(output.model.as_deref(), Some("fixture-chat"));
    assert_eq!(
        output
            .token_usage
            .map(axon_agent_runtime::TokenUsageTelemetry::total_tokens),
        Some(MOCK_TOTAL_TOKENS)
    );
    persistence_tests::assert_completed_child_thread(&config_path, &task_path);
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[test]
fn provider_executor_start_emits_running_lifecycle_before_completion() {
    let config_path = temp_config_path("started-lifecycle");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        config_path.parent().unwrap().join("AGENTS.md"),
        "子代理启动规则",
    )
    .unwrap();
    let (lifecycle_tx, lifecycle_rx) = mpsc::channel();
    let mut environment = fixture_environment(config_path.clone());
    environment.cwd = config_path.parent().unwrap().to_path_buf();
    environment.startup = Arc::new(
        crate::run_commands::RunStartupSnapshot::build(&config_path, &environment.cwd).unwrap(),
    );
    environment.subagent_event_bus = Some(Arc::new(move |event| {
        if let WireEventKind::TurnLifecycle(lifecycle) = event.event.kind {
            lifecycle_tx
                .send(lifecycle)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }));
    let executor = ProviderRunSubagentExecutor::with_provider_turn_runner(
        environment.clone(),
        mock_provider_turn,
    );
    let input = RunSubagentExecutionInput {
        task_path: AgentPath::parse("/root/review").unwrap(),
        message: "审查当前改动".to_owned(),
        fork_context: SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms: 123,
        max_runtime_ms: None,
        model_intent: RunSubagentModelIntent::Inherit,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: false,
    };
    let lease = {
        let mut control = environment.agent_control.lock().unwrap();
        control
            .spawn(axon_agent_runtime::AgentSpawnRequest {
                parent_path: AgentPath::root(),
                task_name: "review".to_owned(),
                message: input.message.clone(),
                nickname: None,
                role: None,
            })
            .unwrap();
        control.lease_execution(&input.task_path).unwrap()
    };
    let session_handle = environment
        .subagent_sessions
        .lock()
        .unwrap()
        .start(axon_agent_runtime::SubagentSessionStartInput {
            parent_session_id: SessionId(1),
            task_path: input.task_path.to_string(),
            forked_from_session_id: Some(SessionId(1)),
            pane_id: axon_domain::PaneId(1),
            cwd: environment.cwd.to_string_lossy().to_string(),
            model_profile_id: ModelProfileId(7),
            permission_snapshot_id: environment.permission_snapshot_id,
            created_at_ms: input.started_at_ms,
            summary: "spawned /root/review".to_owned(),
        })
        .unwrap();
    environment
        .subagent_sessions
        .lock()
        .unwrap()
        .begin_execution_by_task_path(
            &session_handle.task_path,
            lease.execution_id(),
            input.message.clone(),
        )
        .unwrap();
    let completion = RunSubagentCompletionSink::new(
        environment.agent_control.clone(),
        environment.agent_graph_store.clone(),
        environment.subagent_sessions.clone(),
        lease,
    );

    assert!(executor.start(input, completion));

    let lifecycle = lifecycle_rx
        .recv_timeout(FIXTURE_SERVER_TIMEOUT)
        .expect("子代理启动后应立即发出 lifecycle");
    assert_eq!(lifecycle.part_id, "lifecycle:subagent_activity:root:review");
    assert_eq!(lifecycle.kind.as_deref(), Some("subagent_activity"));
    assert_eq!(lifecycle.ui_phase.as_deref(), Some("started"));
    assert_eq!(lifecycle.status, WireLifecycleStatus::Started);
    assert_eq!(lifecycle.started_at_ms, Some(123));
    assert_eq!(lifecycle.tool_name.as_deref(), Some("fixture-chat"));
    assert!(
        lifecycle
            .summary
            .unwrap_or_default()
            .contains("/root/review 已启动")
    );

    let mut progress_events = Vec::new();
    let terminal = loop {
        let event = lifecycle_rx
            .recv_timeout(FIXTURE_ASYNC_COMPLETION_TIMEOUT)
            .expect("子代理 fixture lifecycle 应在进程内完成投影");
        if event.status == WireLifecycleStatus::Completed {
            break event;
        }
        progress_events.push(event);
    };
    assert!(
        progress_events
            .iter()
            .any(|event| event.ui_phase.as_deref() == Some("provider_connecting"))
    );
    assert!(
        progress_events
            .iter()
            .any(|event| event.ui_phase.as_deref() == Some("running"))
    );
    assert!(
        progress_events
            .iter()
            .any(|event| event.ui_phase.as_deref() == Some("provider_generating"))
    );
    let usage = progress_events
        .iter()
        .find(|event| event.total_tokens == Some(MOCK_TOTAL_TOKENS))
        .expect("子代理 provider usage 应在 completion 前投影");
    assert_eq!(usage.total_tokens, Some(MOCK_TOTAL_TOKENS));
    assert_eq!(terminal.status, WireLifecycleStatus::Completed);
    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[test]
fn provider_executor_persists_continuation_and_assembles_cleanly() {
    let config_path = temp_config_path("continuation-e2e");
    fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    fs::write(
        config_path.parent().unwrap().join("AGENTS.md"),
        "子代理启动规则",
    )
    .unwrap();
    let (lifecycle_tx, lifecycle_rx) = mpsc::channel();
    let mut environment = fixture_environment(config_path.clone());
    environment.cwd = config_path.parent().unwrap().to_path_buf();
    environment.startup = Arc::new(
        crate::run_commands::RunStartupSnapshot::build(&config_path, &environment.cwd).unwrap(),
    );
    environment.subagent_event_bus = Some(Arc::new(move |event| {
        if let WireEventKind::TurnLifecycle(lifecycle) = event.event.kind {
            lifecycle_tx
                .send(lifecycle)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }));
    let executor = ProviderRunSubagentExecutor::with_provider_turn_runner(
        environment.clone(),
        mock_provider_turn,
    );
    let task_path = AgentPath::parse("/root/review").unwrap();
    let input = RunSubagentExecutionInput {
        task_path: task_path.clone(),
        message: "审查当前改动".to_owned(),
        fork_context: SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms: 123,
        max_runtime_ms: None,
        model_intent: RunSubagentModelIntent::Inherit,
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
    };
    let lease = {
        let mut control = environment.agent_control.lock().unwrap();
        control
            .spawn(axon_agent_runtime::AgentSpawnRequest {
                parent_path: AgentPath::root(),
                task_name: "review".to_owned(),
                message: input.message.clone(),
                nickname: None,
                role: None,
            })
            .unwrap();
        control.lease_execution(&input.task_path).unwrap()
    };
    let session_handle = environment
        .subagent_sessions
        .lock()
        .unwrap()
        .start(axon_agent_runtime::SubagentSessionStartInput {
            parent_session_id: SessionId(1),
            task_path: input.task_path.to_string(),
            forked_from_session_id: Some(SessionId(1)),
            pane_id: axon_domain::PaneId(1),
            cwd: environment.cwd.to_string_lossy().to_string(),
            model_profile_id: ModelProfileId(7),
            permission_snapshot_id: environment.permission_snapshot_id,
            created_at_ms: input.started_at_ms,
            summary: "spawned /root/review".to_owned(),
        })
        .unwrap();
    environment
        .subagent_sessions
        .lock()
        .unwrap()
        .begin_execution_by_task_path(
            &session_handle.task_path,
            lease.execution_id(),
            input.message.clone(),
        )
        .unwrap();

    let parent_session_id = environment.parent_session_id.clone();
    let store = crate::run_commands::run_agent_store::shared_runtime_store(&config_path).unwrap();
    let continuation_sink =
        crate::run_commands::run_subagent_completion::RunSubagentContinuationSink::open(
            &config_path,
            &parent_session_id,
            &parent_session_id,
        )
        .unwrap();

    let completion = RunSubagentCompletionSink::new(
        environment.agent_control.clone(),
        environment.agent_graph_store.clone(),
        environment.subagent_sessions.clone(),
        lease,
    )
    .with_continuation(continuation_sink);

    assert!(executor.start(input, completion));

    // 等待子代理异步执行完成。
    loop {
        let event = lifecycle_rx
            .recv_timeout(FIXTURE_ASYNC_COMPLETION_TIMEOUT)
            .expect("子代理应在进程内完成");
        if event.status == WireLifecycleStatus::Completed {
            break;
        }
    }

    // continuation 确实持久化进共享库，且引用本身重启后可解析（合同 §5.3.3）。
    let pending = await_pending_continuation(&store, &parent_session_id);
    assert_eq!(
        pending.len(),
        1,
        "子代理完成必须写入 1 条待装配 continuation"
    );
    let record = &pending[0];
    assert_eq!(record.agent_path, "/root/review");
    assert_eq!(record.terminal_kind, "completed");
    assert_eq!(record.exit_reason, "子代理正常结束");
    assert_eq!(record.summary, MOCK_RESPONSE_CONTENT, "摘要是落盘文本");
    assert_eq!(
        record.artifact_path, None,
        "短结果不落盘，摘要级恢复只需要摘要正文"
    );
    assert_eq!(record.token_input, Some(MOCK_INPUT_TOKENS));
    assert_eq!(record.token_output, Some(MOCK_OUTPUT_TOKENS));
    assert_eq!(record.token_reasoning, Some(0));
    assert_eq!(
        record.idempotency_key,
        format!(
            "continuation:{parent_session_id}:{}:completed",
            record.child_session_id
        ),
        "幂等键必须由跨进程稳定的子会话 ID 构成，不得含随进程消亡的 job_id"
    );
    assert_eq!(
        record.job_ref, None,
        "本用例的完成回执未绑定临时 Job；job_ref 只作可读线索，缺失不影响装配"
    );
    // 合同 §8.2.2：同一 continuation 最多产生一次有效执行。
    assert!(
        axon_session::AgentContinuationStore::consume_continuation(
            &store,
            &record.continuation_id,
            "run:test",
            2_000,
        )
        .unwrap(),
        "第一次装配就是那唯一一次有效执行"
    );
    assert!(
        !axon_session::AgentContinuationStore::consume_continuation(
            &store,
            &record.continuation_id,
            "tui:test",
            2_001,
        )
        .unwrap(),
        "重复装配必须拿到 false 并放弃"
    );

    let _ = fs::remove_dir_all(config_path.parent().unwrap());
}

#[path = "run_subagent_executor_lifecycle_tests.rs"]
mod lifecycle_tests;

#[path = "run_subagent_persistence_tests.rs"]
mod persistence_tests;
