use super::*;

use axon_domain::{
    AudienceContext, AudienceKind, ConsumerCursor, DisclosurePolicy, InteractionId,
    InteractionRecord, InteractionSource, InteractionStatus, ReplyRoute, ScopeRef,
};
use axon_protocol::{
    DAEMON_PROTOCOL_VERSION, DaemonCommand, DaemonRequest, SubmitInteractionRequest,
};
use axon_session::{
    CheckpointTelemetry, SessionEventRecord, SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{
    ActionExecutionRequest, RuntimeEvent, TurnEventSink, TurnExecutionError, TurnExecutionOutcome,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const TEST_NOW_MS: u64 = 42;
const LIVE_WIRE_EVENT_TYPE: &str = "wire_event";
const TURN_COMPLETED_EVENT_TYPE: &str = "turn_completed";
const LIVE_WIRE_EVENT_PAYLOAD: &str =
    r#"{"kind":{"type":"providerStatus","payload":{"phase":"connecting"}},"recordedAtMs":42}"#;
const LIVE_COMMIT_NOW_MS: u64 = 142;
const TEST_LEASE_DURATION_MS: u64 = 1_000;
const TEST_TICK_LIMIT: u32 = 10;
const TEST_WAIT_DURATION: Duration = Duration::from_secs(2);
const TEST_POLL_INTERVAL: Duration = Duration::from_millis(10);
const TEST_PROTOCOL_CLIENT: &str = "daemon-interaction-test";

#[derive(Clone, Copy)]
struct NoopExecutor;

impl TurnExecutor for NoopExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: Vec::new(),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Clone)]
struct LiveProjectionExecutor(Arc<AtomicU64>);

impl TurnExecutor for LiveProjectionExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Err(TurnExecutionError::permanent(
            "Primary Turn 必须走实时投影通道",
        ))
    }

    fn execute_streaming(
        &self,
        _request: ActionExecutionRequest,
        sink: Option<&dyn TurnEventSink>,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.0.store(LIVE_COMMIT_NOW_MS, Ordering::Release);
        sink.expect("Primary Turn 必须注入实时投影通道")
            .publish(&RuntimeEvent {
                event_type: LIVE_WIRE_EVENT_TYPE.to_owned(),
                payload_json: LIVE_WIRE_EVENT_PAYLOAD.to_owned(),
            })
            .expect("实时投影事件必须落库");
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: TURN_COMPLETED_EVENT_TYPE.to_owned(),
                payload_json: serde_json::json!({"response": "实时回答"}).to_string(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        TEST_NOW_MS
    }
}

#[derive(Clone)]
struct SteppedClock(Arc<AtomicU64>);

impl RuntimeClock for SteppedClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy)]
struct TestProcess;

impl ProcessIdentity for TestProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[test]
fn ipc_submit_is_durable_and_waits_for_primary_turn_worker() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let interaction = interaction(workspace.workspace_id.as_str());
    let mut host = test_host(store.clone());
    host.start().expect("启动 Host");

    let submit = host.handle_request(request(
        "submit",
        DaemonCommand::SubmitInteraction(SubmitInteractionRequest {
            interaction: interaction.clone(),
        }),
    ));
    assert!(submit[0].ok);

    let query = host.handle_request(request(
        "query",
        DaemonCommand::GetInteraction {
            interaction_id: interaction.interaction_id.as_str().to_owned(),
        },
    ));
    let stored: Option<InteractionRecord> =
        serde_json::from_value(query[0].body.clone()).expect("解析查询结果");
    assert_eq!(
        stored.expect("Interaction 已持久化").status,
        InteractionStatus::Received
    );
}

#[test]
fn primary_turn_projects_live_event_before_completion() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let interaction = interaction(workspace.workspace_id.as_str());
    let shared_now = Arc::new(AtomicU64::new(TEST_NOW_MS));
    let mut host = live_projection_host(
        store.clone(),
        SteppedClock(Arc::clone(&shared_now)),
        LiveProjectionExecutor(Arc::clone(&shared_now)),
    );
    host.start().expect("启动 Host");

    let submit = host.handle_request(request(
        "submit-live",
        DaemonCommand::SubmitInteraction(SubmitInteractionRequest {
            interaction: interaction.clone(),
        }),
    ));
    assert!(submit[0].ok);
    // Daemon 运行时由独立协调线程驱动主 turn；测试显式驱动同一入口。
    host.primary_turn_tick(TEST_TICK_LIMIT)
        .expect("驱动主 turn");

    let session_id = primary_session_id_for_interaction(&interaction).expect("主会话 ID");
    // 主 turn 在 worker pool 上执行：轮询等待实时投影与收口事件落库。
    let deadline = Instant::now() + TEST_WAIT_DURATION;
    let events = loop {
        let events = store
            .replay_events(&session_id, 0, 16)
            .expect("读取 Session 事件");
        // 本轮的三条 durable 事实：用户消息、实时投影、收口。
        if events.len() >= 3 || Instant::now() >= deadline {
            break events;
        }
        thread::sleep(TEST_POLL_INTERVAL);
    };
    assert_eq!(
        events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>(),
        [
            LIVE_WIRE_EVENT_TYPE,
            LIVE_WIRE_EVENT_TYPE,
            TURN_COMPLETED_EVENT_TYPE
        ],
        "用户事实与实时投影必须早于收口批量事件落库"
    );
    let live_payload: serde_json::Value =
        serde_json::from_str(&events[1].payload_json).expect("解析实时 wire payload");
    let expected_payload: serde_json::Value =
        serde_json::from_str(LIVE_WIRE_EVENT_PAYLOAD).expect("解析期望 wire payload");
    assert_eq!(
        live_payload.get("kind"),
        expected_payload.get("kind"),
        "实时 wire 事件内容必须保持不变"
    );
    assert_eq!(
        live_payload
            .get("interactionId")
            .and_then(serde_json::Value::as_str),
        Some(interaction.interaction_id.as_str())
    );
    assert_eq!(
        live_payload
            .get("recordedAtMs")
            .and_then(serde_json::Value::as_u64),
        Some(TEST_NOW_MS),
        "wire payload 必须保留业务事件时间"
    );
    assert_eq!(
        events[1].emitted_at_ms, LIVE_COMMIT_NOW_MS,
        "durable commit 时间必须取自提交前时钟"
    );
}

#[test]
fn query_activity_command_returns_scoped_snapshot() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    store.initialize().expect("初始化 store");
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let mut host = test_host(store);
    host.start().expect("启动 Host");

    let response = host.handle_request(request(
        "query-activity",
        DaemonCommand::QueryActivity(axon_domain::ActivityQuery {
            scope: ScopeRef::workspace(workspace.workspace_id.as_str()).expect("有效 scope"),
            goal_limit: 10,
            result_limit: 10,
            event_after_sequence: 0,
            event_limit: 20,
        }),
    ));
    assert_eq!(response.len(), 1);
    assert!(response[0].ok);
    let snapshot: axon_domain::ActivitySnapshot =
        serde_json::from_value(response[0].body.clone()).expect("反序列化 ActivitySnapshot");
    assert!(snapshot.active_goals.is_empty());
    assert!(snapshot.unread_results.is_empty());
}

fn live_projection_host(
    store: SqliteRuntimeStore,
    clock: SteppedClock,
    executor: LiveProjectionExecutor,
) -> DaemonHost<SqliteRuntimeStore, LiveProjectionExecutor, NoopPublisher, SteppedClock, TestProcess>
{
    DaemonHost::new(
        store,
        executor,
        NoopPublisher,
        clock,
        TestProcess,
        RuntimeHostId::new("daemon-live-projection-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    )
}

fn test_host(
    store: SqliteRuntimeStore,
) -> DaemonHost<SqliteRuntimeStore, NoopExecutor, NoopPublisher, FixedClock, TestProcess> {
    DaemonHost::new(
        store,
        NoopExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new("daemon-interaction-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    )
}

fn request(request_id: &str, command: DaemonCommand) -> DaemonRequest {
    DaemonRequest {
        message_type: "command".to_owned(),
        request_id: request_id.to_owned(),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: TEST_PROTOCOL_CLIENT.to_owned(),
        session_id: None,
        command,
    }
}

fn interaction(workspace_id: &str) -> InteractionRecord {
    InteractionRecord::new(
        InteractionId::new("interaction-ipc-query").expect("Interaction ID"),
        InteractionSource::new("cli", "message-ipc-query").expect("来源"),
        ScopeRef::workspace(workspace_id).expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "terminal").expect("回复路由"),
        "检查 durable 状态",
        TEST_NOW_MS,
    )
    .expect("Interaction")
}
