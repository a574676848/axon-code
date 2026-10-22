use super::super::interaction_turn_runner::primary_session_id_for_interaction;
use super::super::queued_action;
use super::*;
use axon_domain::{
    AudienceContext, AudienceKind, ConsumerCursor, DaemonSessionId, DisclosurePolicy, ExecutionId,
    ExecutionRecord, ExecutionStatus, GoalId, GoalPlan, GoalRecord, GoalStatus, InteractionId,
    InteractionRecord, InteractionSource, ReplyRoute, RuntimeHostId, ScopeRef,
};
use axon_session::{
    CheckpointTelemetry, ExecutionStore, GoalStore, InteractionStore, LedgerEventStore,
    SessionEventRecord, SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{
    ActionExecutionRequest, JarvisRuntimeSelector, TurnExecutionError, TurnExecutionOutcome,
    model::RuntimeEvent,
};
use std::thread;
use std::{
    sync::{
        Arc, Condvar, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

const TEST_NOW_MS: u64 = 10;
const TEST_LEASE_DURATION_MS: u64 = 1_000;
const TEST_WAIT_DURATION: Duration = Duration::from_secs(2);
const TEST_POLL_INTERVAL: Duration = Duration::from_millis(10);
const USER_INPUT_ACTION_KIND: &str = "user_input";

#[derive(Clone)]
struct PanicOnceExecutor {
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<ActionExecutionRequest>>>,
    first_started: Sender<()>,
    first_release: Arc<Mutex<Receiver<()>>>,
}

impl TurnExecutor for PanicOnceExecutor {
    fn execute(
        &self,
        request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first_started.send(()).expect("通知首个 Action 已开始");
            self.first_release
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .recv_timeout(TEST_WAIT_DURATION)
                .expect("释放首个 Action");
            panic!("模拟旧 epoch Worker panic");
        }
        Ok(TurnExecutionOutcome {
            events: Vec::new(),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
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

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AnswerExecutor {
    calls: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<ActionExecutionRequest>>>,
}

impl TurnExecutor for AnswerExecutor {
    fn execute(
        &self,
        request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: "turn_completed".to_owned(),
                payload_json: r#"{"response":"4"}"#.to_owned(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Default)]
struct ConcurrentExecutionState {
    started: usize,
    released: bool,
}

#[derive(Clone, Default)]
struct ConcurrentExecutor {
    state: Arc<(Mutex<ConcurrentExecutionState>, Condvar)>,
}

impl ConcurrentExecutor {
    fn wait_until_started(&self, expected: usize) {
        let (lock, condition) = &*self.state;
        let state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, timeout) = condition
            .wait_timeout_while(state, TEST_WAIT_DURATION, |state| state.started < expected)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!timeout.timed_out(), "等待并发 Primary lane 启动超时");
        assert_eq!(state.started, expected);
    }

    fn release(&self) {
        let (lock, condition) = &*self.state;
        lock.lock().unwrap_or_else(PoisonError::into_inner).released = true;
        condition.notify_all();
    }
}

impl TurnExecutor for ConcurrentExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        let (lock, condition) = &*self.state;
        let mut state = lock.lock().unwrap_or_else(PoisonError::into_inner);
        state.started += 1;
        condition.notify_all();
        while !state.released {
            state = condition
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        Ok(TurnExecutionOutcome {
            events: Vec::new(),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[test]
fn daemon_primary_turn_answers_plain_question_without_creating_task() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace = register_workspace(&store, directory.path());
    let executor = AnswerExecutor::default();
    let interaction = interaction(
        "plain-question",
        "plain-question-message",
        &workspace.workspace_id,
        "2 + 2 等于多少？",
    );
    store.append(&interaction).expect("持久化 Interaction");
    let mut host = test_host(store.clone(), executor.clone());
    host.start().expect("启动 Host");
    let session_id = primary_session_id_for_interaction(&interaction).expect("Primary Session ID");
    let subscription = host
        .subscribe_events(session_id, interaction.interaction_id.clone(), None)
        .expect("订阅 Primary 事件");

    host.primary_turn_tick(10).expect("运行主 turn");

    assert_eq!(executor.calls.load(Ordering::SeqCst), 1);
    let requests = executor
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests[0].context.cwd, workspace.workspace_root);
    assert_eq!(
        requests[0].runtime_profile,
        JarvisRuntimeSelector::for_primary_turn()
    );
    let crate::application::event_stream_bridge::StreamNotification::SessionEvent(streamed) =
        subscription.try_recv().expect("收到 Primary 实时事件")
    else {
        panic!("应收到 Primary Session 事件");
    };
    // 第一条推送是本轮的用户消息事实，随后才是收口事件。
    assert_eq!(streamed.event.event_type, "wire_event");
    let crate::application::event_stream_bridge::StreamNotification::SessionEvent(streamed) =
        subscription.try_recv().expect("收到 Primary 收口事件")
    else {
        panic!("应收到 Primary 收口事件");
    };
    assert_eq!(streamed.event.event_type, "turn_completed");
    // 本轮 durable 事实依次是：用户消息、收口事件。
    let replayed = store
        .replay_events(&streamed.event.session_id, 0, 10)
        .expect("读取 durable Session 事件");
    assert_eq!(replayed.len(), 2);
    let user_payload: serde_json::Value =
        serde_json::from_str(&replayed[0].payload_json).expect("解析用户消息 payload");
    assert_eq!(
        user_payload.get("kind").and_then(|kind| kind.get("type")),
        Some(&serde_json::Value::String("userMessage".to_owned())),
        "首条事实必须是本轮的用户消息"
    );
    assert_eq!(replayed[1], streamed.event);
    assert!(store.list_active_goals(10).expect("读取 Goal").is_empty());
    let execution_id = primary_execution_id(&interaction);
    assert!(
        store
            .get_execution(&execution_id)
            .expect("读取 Execution")
            .is_none()
    );
    assert!(
        store
            .replay_ledger_events(0, 10)
            .expect("读取 Event")
            .is_empty()
    );
    assert_terminal_interaction(&store);
}

fn interaction(
    interaction_id: &str,
    source_message_id: &str,
    workspace_id: &str,
    content: &str,
) -> InteractionRecord {
    InteractionRecord::new(
        InteractionId::new(interaction_id).expect("Interaction ID"),
        InteractionSource::new("cli", source_message_id).expect("来源"),
        ScopeRef::workspace(workspace_id).expect("作用域"),
        AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        ReplyRoute::new("cli", "terminal").expect("回复路由"),
        content,
        TEST_NOW_MS,
    )
    .expect("Interaction")
}

fn register_workspace(
    store: &SqliteRuntimeStore,
    root: &std::path::Path,
) -> axon_session::WorkspaceRecord {
    let workspace = WorkspaceResolver
        .resolve(root, TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    workspace
}

fn test_host<E: TurnExecutor + Clone + Send + Sync + 'static>(
    store: SqliteRuntimeStore,
    executor: E,
) -> DaemonHost<SqliteRuntimeStore, E, NoopPublisher, FixedClock, TestProcess> {
    DaemonHost::new(
        store,
        executor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new("daemon-primary-turn-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    )
}

fn primary_execution_id(interaction: &InteractionRecord) -> axon_domain::ExecutionId {
    axon_domain::ExecutionId::new(
        super::super::interaction_turn_runner::stable_interaction_id(
            "execution-",
            interaction.interaction_id.as_str(),
        ),
    )
    .expect("Execution ID")
}

fn assert_terminal_interaction(store: &SqliteRuntimeStore) {
    assert!(
        store
            .claim_processing(10, TEST_NOW_MS + 1, TEST_LEASE_DURATION_MS)
            .expect("检查 Interaction")
            .is_empty()
    );
}

#[test]
fn reset_pending_action_survives_previous_worker_panic_without_daemon_restart() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let (started_sender, started_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let executor = PanicOnceExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
        requests: Arc::new(Mutex::new(Vec::new())),
        first_started: started_sender,
        first_release: Arc::new(Mutex::new(release_receiver)),
    };
    let host_id = RuntimeHostId::new("daemon-worker-test-host").expect("Host ID");
    let session_id = DaemonSessionId::new("daemon-worker-test-session").expect("Session ID");
    let mut host = DaemonHost::new(
        store.clone(),
        executor.clone(),
        NoopPublisher,
        FixedClock,
        TestProcess,
        host_id,
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    );
    host.start().expect("启动 Host");
    host.create_session(session_id.clone(), "old-cwd")
        .expect("创建 Session");
    store
        .enqueue_action(
            &test_action(&session_id, "old-action", "旧请求"),
            TEST_NOW_MS,
        )
        .expect("旧 Action 入队");
    host.wake_session(session_id.clone());
    started_receiver
        .recv_timeout(TEST_WAIT_DURATION)
        .expect("旧 Action 开始执行");

    let reset = host
        .reset_session(&session_id, "new-cwd")
        .expect("重置 Session");
    let submitted = store
        .enqueue_action(
            &test_action(&session_id, "new-action", "新请求"),
            TEST_NOW_MS,
        )
        .expect("新 Action 入队");
    host.wake_session(session_id.clone());
    release_sender.send(()).expect("触发旧 Worker panic");

    wait_until(|| {
        store
            .list_pending_actions(&session_id, 10)
            .is_ok_and(|actions| actions.is_empty())
    });
    let requests = executor
        .requests
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].action.action_id, submitted.action_id);
    assert_eq!(requests[1].context.run_session_id, reset.run_session_id);
    assert_eq!(requests[1].context.cwd, reset.cwd);
    assert_eq!(requests[1].context.context_version, reset.context_version);
}

fn test_action(
    session_id: &DaemonSessionId,
    action_id: &str,
    prompt: &str,
) -> axon_session::SessionActionRecord {
    queued_action(
        SessionActionId::new(action_id).expect("Action ID"),
        session_id.clone(),
        USER_INPUT_ACTION_KIND.to_owned(),
        serde_json::json!({"prompt": prompt}).to_string(),
    )
}

fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + TEST_WAIT_DURATION;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(TEST_POLL_INTERVAL);
    }
    assert!(predicate(), "等待 Worker 提交新 Action 超时");
}

#[path = "daemon_host_worker_goal_flow_tests.rs"]
mod daemon_host_worker_goal_flow_tests;
