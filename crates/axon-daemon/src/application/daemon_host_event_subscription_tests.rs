use super::super::event_stream_bridge::EventStreamBridge;
use super::*;
use axon_domain::{
    AudienceContext, AudienceKind, DisclosurePolicy, InteractionRecord, InteractionSource,
    ReplyRoute, RuntimeHostId, ScopeRef,
};
use axon_session::{
    CheckpointTelemetry, InteractionStore, SessionEventRecord, SqliteRuntimeStore,
    WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};

const TEST_NOW_MS: u64 = 100;
const TEST_LEASE_MS: u64 = 1_000;
const TEST_EVENT_TIMEOUT: Duration = Duration::from_millis(10);

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

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        TEST_NOW_MS
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
fn full_replay_page_completes_connection_before_cursor_resubscribe() {
    let bridge = EventStreamBridge::new();
    let session_id = DaemonSessionId::new("full-replay-session").expect("Session ID");
    let interaction_id = InteractionId::new("full-replay-interaction").expect("Interaction ID");
    let subscription = bridge
        .subscribe(session_id.clone(), interaction_id, None)
        .expect("建立订阅");
    let mut replay_event = session_event(session_id);
    let events = (0..DEFAULT_QUEUE_LIMIT)
        .map(|sequence| {
            replay_event.sequence = u64::from(sequence) + 1;
            replay_event.clone()
        })
        .collect();

    let dispatch = subscription_dispatch(OpenedEventSubscription {
        request_id: "full-replay-request".to_owned(),
        interaction_status: InteractionStatus::Processing,
        events,
        page_full: true,
        snapshot_required: false,
        replay_cursor: Some(ConsumerCursor::new(
            "generation",
            DEFAULT_QUEUE_LIMIT as u64,
        )),
        subscription,
    });

    assert!(matches!(dispatch, IpcDispatch::Complete(_)));
    assert_eq!(bridge.subscriber_count(), 0);
}

#[test]
fn subscription_ready_exposes_replay_cursor_for_empty_replay() {
    let bridge = EventStreamBridge::new();
    let session_id = DaemonSessionId::new("ready-cursor-session").expect("Session ID");
    let interaction_id = InteractionId::new("ready-cursor-interaction").expect("Interaction ID");
    let subscription = bridge
        .subscribe(session_id.clone(), interaction_id, None)
        .expect("建立订阅");
    let cursor = ConsumerCursor::new("generation", 7);

    let dispatch = subscription_dispatch(OpenedEventSubscription {
        request_id: "ready-cursor-request".to_owned(),
        interaction_status: InteractionStatus::Processing,
        events: Vec::new(),
        page_full: false,
        snapshot_required: false,
        replay_cursor: Some(cursor.clone()),
        subscription,
    });
    let IpcDispatch::Subscription { initial, .. } = dispatch else {
        panic!("空 replay 仍应保持实时订阅");
    };

    assert_eq!(initial[0].body["event"], SUBSCRIPTION_READY_EVENT);
    assert_eq!(
        initial[0].body["replayCursor"],
        serde_json::to_value(cursor).expect("序列化 cursor")
    );
}

#[test]
fn subscription_pushes_events_and_terminal_status_without_client_polling() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let interaction = interaction(workspace.workspace_id.as_str());
    store.append(&interaction).expect("持久化 Interaction");
    let session_id = primary_session_id_for_interaction(&interaction).expect("Session ID");
    let mut host = test_host(store.clone());
    host.start().expect("启动 Host");
    let bridge = host.stream_bridge();

    let dispatch = host.handle_ipc_request(subscription_request(
        &session_id,
        interaction.interaction_id.as_str(),
    ));
    let IpcDispatch::Subscription {
        initial,
        mut stream,
    } = dispatch
    else {
        panic!("应建立实时订阅");
    };
    assert_eq!(initial[0].body["event"], SUBSCRIPTION_READY_EVENT);
    assert_eq!(initial[0].body["events"], serde_json::json!([]));
    assert_eq!(
        initial[0].body["replayCursor"]["generation"],
        host.gateway_dependencies()
            .host_generation
            .expect("Host generation")
    );
    assert_eq!(initial[0].body["replayCursor"]["sequence"], 0);

    let mut event = session_event(session_id.clone());
    event.generation = host
        .gateway_dependencies()
        .host_generation
        .expect("Host generation");
    bridge.publish(&event, &event.cursor());
    let pushed = stream
        .recv_timeout(TEST_EVENT_TIMEOUT)
        .expect("读取推送")
        .expect("应收到事件");
    assert_eq!(pushed.body["event"], SESSION_EVENT);
    assert_eq!(pushed.body["record"]["sequence"], 1);

    reject_interaction_and_assert_terminal(&interaction, &bridge, stream.as_mut());
}

#[test]
fn subscription_receives_runtime_observation_without_advancing_cursor() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let interaction = interaction(workspace.workspace_id.as_str());
    store.append(&interaction).expect("持久化 Interaction");
    let mut host = test_host(store);
    host.start().expect("启动 Host");
    let bridge = host.stream_bridge();

    let session_id = primary_session_id_for_interaction(&interaction).expect("Session ID");
    let dispatch = host.handle_ipc_request(subscription_request(
        &session_id,
        interaction.interaction_id.as_str(),
    ));
    let IpcDispatch::Subscription { mut stream, .. } = dispatch else {
        panic!("应建立实时订阅");
    };

    let observation = axon_domain::RuntimeObservationFrame::new(
        TEST_NOW_MS,
        session_id.as_str(),
        interaction.interaction_id.as_str(),
    )
    .with_lsp(axon_domain::LspMetricsSnapshot {
        active_clients: 3,
        pending_requests: 0,
        diagnostics_count: 0,
        last_latency_ms: 5,
    });
    bridge.publish_observation(&session_id, &observation);

    let pushed = stream
        .recv_timeout(TEST_EVENT_TIMEOUT)
        .expect("读取推送")
        .expect("应收到事件");
    assert_eq!(pushed.body["event"], RUNTIME_OBSERVATION_EVENT);
    assert_eq!(pushed.body["frame"]["lsp"]["activeClients"], 3);
    assert!(pushed.cursor.is_none(), "非持久观测帧不得携带游标");
}

#[test]
fn subscription_rejects_session_from_another_interaction() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let interaction = interaction(workspace.workspace_id.as_str());
    store.append(&interaction).expect("持久化 Interaction");
    let mut host = test_host(store);
    host.start().expect("启动 Host");

    let dispatch = host.handle_ipc_request(subscription_request(
        &DaemonSessionId::new("wrong-session").expect("Session ID"),
        interaction.interaction_id.as_str(),
    ));
    let IpcDispatch::Complete(responses) = dispatch else {
        panic!("错误绑定不得建立订阅");
    };
    assert!(!responses[0].ok);
}

#[test]
fn replay_cursor_filters_overlapping_live_event() {
    let bridge = EventStreamBridge::new();
    let session_id = DaemonSessionId::new("overlap-session").expect("Session ID");
    let interaction_id = InteractionId::new("overlap-interaction").expect("Interaction ID");
    let subscription = bridge
        .subscribe(session_id.clone(), interaction_id, None)
        .expect("订阅应成功");
    let replayed = session_event(session_id);
    bridge.publish(&replayed, &replayed.cursor());
    let mut stream = SessionEventResponseStream {
        request_id: "overlap-request".to_owned(),
        replay_cursor: Some(replayed.cursor()),
        subscription,
        pending_response: None,
    };

    assert!(
        stream
            .recv_timeout(TEST_EVENT_TIMEOUT)
            .expect("重复事件应被忽略")
            .is_none()
    );
}

#[test]
fn subscription_accepts_live_event_after_generation_change() {
    let bridge = EventStreamBridge::new();
    let session_id = DaemonSessionId::new("cross-generation-session").expect("Session ID");
    let interaction_id = InteractionId::new("interaction-a").expect("Interaction ID");
    let subscription = bridge
        .subscribe(session_id.clone(), interaction_id, None)
        .expect("建立订阅");
    let mut stream = SessionEventResponseStream {
        request_id: "cross-generation-request".to_owned(),
        replay_cursor: Some(ConsumerCursor::new("generation-a", 4)),
        subscription,
        pending_response: None,
    };
    let event = SessionEventRecord {
        session_id,
        sequence: 5,
        generation: "generation-b".to_owned(),
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"interactionId":"interaction-a"}"#.to_owned(),
        emitted_at_ms: TEST_NOW_MS,
    };
    bridge.publish(&event, &event.cursor());

    let response = stream
        .recv_timeout(TEST_EVENT_TIMEOUT)
        .expect("应读取跨 generation 事件")
        .expect("应收到事件");
    assert_eq!(response.body["record"]["sequence"], 5);
}

#[test]
fn subscription_converts_generation_reset_to_ipc_stream_reset() {
    let bridge = EventStreamBridge::new();
    let session_id = DaemonSessionId::new("reset-ipc-session").expect("Session ID");
    let interaction_id = InteractionId::new("reset-ipc-interaction").expect("Interaction ID");
    let last_cursor = ConsumerCursor::new("generation-a", 4);
    let subscription = bridge
        .subscribe(
            session_id.clone(),
            interaction_id,
            Some(last_cursor.clone()),
        )
        .expect("建立订阅");
    let mut stream = SessionEventResponseStream {
        request_id: "reset-ipc-request".to_owned(),
        replay_cursor: Some(last_cursor.clone()),
        subscription,
        pending_response: None,
    };
    let event = SessionEventRecord {
        session_id,
        sequence: 5,
        generation: "generation-b".to_owned(),
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"interactionId":"reset-ipc-interaction"}"#.to_owned(),
        emitted_at_ms: TEST_NOW_MS,
    };

    bridge.publish(&event, &event.cursor());
    let response = stream
        .recv_timeout(TEST_EVENT_TIMEOUT)
        .expect("读取 generation reset")
        .expect("应收到 IPC reset");

    assert_eq!(response.body["event"], STREAM_RESET_EVENT);
    assert_eq!(response.body["reason"], "generation_change");
    assert_eq!(
        response.body["cursor"],
        serde_json::to_value(last_cursor).expect("序列化 last cursor")
    );
    assert_eq!(bridge.subscriber_count(), 0);
}

#[test]
fn subscription_batches_ready_events_and_keeps_status_as_separate_boundary() {
    let bridge = EventStreamBridge::new();
    let session_id = DaemonSessionId::new("batch-session").expect("Session ID");
    let interaction_id = InteractionId::new("subscription-interaction").expect("Interaction ID");
    let subscription = bridge
        .subscribe(session_id.clone(), interaction_id.clone(), None)
        .expect("建立订阅");
    let mut stream = SessionEventResponseStream {
        request_id: "batch-request".to_owned(),
        replay_cursor: None,
        subscription,
        pending_response: None,
    };
    let first = session_event(session_id.clone());
    bridge.publish_for_interaction(&interaction_id, &first, &first.cursor());
    let mut second = first.clone();
    second.sequence = 2;
    bridge.publish_for_interaction(&interaction_id, &second, &second.cursor());
    bridge.publish_interaction_status(&session_id, &interaction_id, InteractionStatus::Rejected);
    let mut terminal_event = second.clone();
    terminal_event.sequence = 3;
    terminal_event.event_type = "turn_completed".to_owned();
    bridge.publish_for_interaction(&interaction_id, &terminal_event, &terminal_event.cursor());

    let batch = stream
        .recv_timeout_batch(TEST_EVENT_TIMEOUT)
        .expect("读取批次")
        .expect("应收到文本事件");
    assert_eq!(batch.len(), 2);
    assert!(
        batch
            .iter()
            .all(|response| response.body["event"] == SESSION_EVENT)
    );

    let status = stream
        .recv_timeout_batch(TEST_EVENT_TIMEOUT)
        .expect("读取状态")
        .expect("应收到终态");
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].body["event"], INTERACTION_STATUS_EVENT);

    let terminal = stream
        .recv_timeout_batch(TEST_EVENT_TIMEOUT)
        .expect("读取终态事件")
        .expect("应收到 turn_completed");
    assert_eq!(terminal.len(), 1);
    assert_eq!(terminal[0].body["record"]["eventType"], "turn_completed");
}

#[test]
fn subscription_replay_ignores_generation_for_durable_cursor() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = WorkspaceResolver
        .resolve(directory.path(), TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    let session_id =
        primary_session_id_for_interaction(&interaction(workspace.workspace_id.as_str()))
            .expect("Session ID");
    let mut host = test_host(store);
    host.start().expect("启动 Host");
    host.create_session(
        session_id.clone(),
        directory.path().to_string_lossy().as_ref(),
    )
    .expect("创建 Session");

    let cursor = ConsumerCursor::new("previous-generation", 7);
    let snapshot = attach_from_cursor(&host, &session_id, Some(cursor)).expect("读取 replay");
    assert!(!snapshot.snapshot_required);
}

fn reject_interaction_and_assert_terminal(
    interaction: &InteractionRecord,
    bridge: &EventStreamBridge,
    stream: &mut dyn IpcResponseStream,
) {
    bridge.publish_interaction_status(
        &primary_session_id_for_interaction(interaction).expect("Primary Session ID"),
        &interaction.interaction_id,
        InteractionStatus::Rejected,
    );
    let terminal = stream
        .recv_timeout(TEST_EVENT_TIMEOUT)
        .expect("读取终态")
        .expect("应收到终态");
    assert_eq!(terminal.body["event"], INTERACTION_STATUS_EVENT);
    assert_eq!(terminal.body["status"], "rejected");
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
        RuntimeHostId::new("subscription-host").expect("Host ID"),
        TEST_LEASE_MS,
        TEST_LEASE_MS,
    )
}

fn subscription_request(session_id: &DaemonSessionId, interaction_id: &str) -> DaemonRequest {
    DaemonRequest {
        message_type: "command".to_owned(),
        request_id: "subscribe-request".to_owned(),
        protocol_version: DAEMON_PROTOCOL_VERSION,
        client_id: "subscription-test".to_owned(),
        session_id: None,
        command: DaemonCommand::SubscribeEvents {
            session_id: session_id.as_str().to_owned(),
            interaction_id: interaction_id.to_owned(),
            resume_cursor: None,
        },
    }
}

fn interaction(workspace_id: &str) -> InteractionRecord {
    InteractionRecord::new(
        InteractionId::new("subscription-interaction").expect("Interaction ID"),
        InteractionSource::new("cli", "subscription-message").expect("来源"),
        ScopeRef::workspace(workspace_id).expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "terminal").expect("回复路由"),
        "验证发布订阅",
        TEST_NOW_MS,
    )
    .expect("Interaction")
}

fn session_event(session_id: DaemonSessionId) -> SessionEventRecord {
    SessionEventRecord {
        session_id,
        sequence: 1,
        generation: "generation-a".to_owned(),
        event_type: "wire_event".to_owned(),
        payload_json: r#"{"interactionId":"subscription-interaction"}"#.to_owned(),
        emitted_at_ms: TEST_NOW_MS,
    }
}
