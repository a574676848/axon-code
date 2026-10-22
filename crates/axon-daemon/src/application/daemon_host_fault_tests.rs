//! Daemon 故障注入验收（R7-04）。
//!
//! 覆盖 Provider 超时与 SQLite busy 两类注入故障，共同验收标准是「无永久
//! running/claimed」：故障之后 Execution 必须回到可推进状态或结论态，不得留下幽灵
//! lease。进程中断（kill）的等价验收见
//! `supervisor_tick_quarantines_interrupted_execution_before_reclaim` 与中断隔离用例，
//! 投递失败的等价验收见远控投递 soak 报告。

use super::*;
use axon_domain::{
    ConsumerCursor, DecisionEffect, DecisionId, DecisionRecord, ExecutionId, ExecutionRecord,
    ExecutionStatus, GoalId, GoalPlan, GoalRecord, GoalStatus, ScopeRef, SessionActionId,
    SessionActionStatus,
};
use axon_session::{
    CheckpointTelemetry, DecisionStore, ExecutionStore, GoalStore, HealthStore,
    SessionActionRecord, SessionEventRecord, SqliteRuntimeStore,
};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const FAULT_BASE_NOW_MS: u64 = 2_000_000;
/// Host 与 Action 租约都远长于本轮时钟推进，避免把「过期接管」混进本次故障注入。
const FAULT_LEASE_MS: u64 = 300_000;
/// 仅用于越过可重试退避，必须小于租约窗口。
const FAULT_RETRY_ADVANCE_MS: u64 = 60_000;
const FAULT_QUIESCE_TIMEOUT: Duration = Duration::from_secs(20);
const FAULT_POLL_INTERVAL: Duration = Duration::from_millis(1);
/// 写锁先被占用、随后释放的观察窗口。
const FAULT_BLOCKED_WINDOW: Duration = Duration::from_millis(200);
/// 故障注入使用的写锁等待上限：远小于生产上限，避免用例空转等待固定超时。
const FAULT_BUSY_TIMEOUT: Duration = Duration::from_millis(200);
/// busy 失败的耗时上界；只验证「有界失败」，不与实现中的超时数值耦合。
const FAULT_BUSY_FAILURE_BOUND: Duration = Duration::from_secs(2);
const TRANSIENT_EXECUTION_STATUSES: [&str; 2] = ["running", "claimed"];
const FAULT_WORKER_OWNER: &str = "fault-worker";

#[derive(Clone)]
struct TimeoutOnceExecutor {
    calls: Arc<AtomicU32>,
}

impl TimeoutOnceExecutor {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicU32::new(0)),
        }
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl TurnExecutor for TimeoutOnceExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        // 首次调用注入 Provider 超时；可重试错误必须回到队列，而不是留在 running。
        if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(TurnExecutionError::retryable("Provider 请求超时"));
        }
        Ok(TurnExecutionOutcome {
            events: Vec::new(),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Clone)]
struct FaultClock(Arc<AtomicU64>);

impl FaultClock {
    fn new(now_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now_ms)))
    }

    fn advance_by(&self, delta_ms: u64) {
        self.0.fetch_add(delta_ms, Ordering::AcqRel);
    }
}

impl RuntimeClock for FaultClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy)]
struct FaultProcess;

impl ProcessIdentity for FaultProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Copy, Default)]
struct FaultPublisher;

impl EventPublisher for FaultPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn provider_timeout_requeues_without_permanent_running_and_retries_after_backoff() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let clock = FaultClock::new(FAULT_BASE_NOW_MS);
    let executor = TimeoutOnceExecutor::new();
    let mut host = fault_host(store.clone(), executor.clone(), clock.clone());
    host.start().expect("启动 Host");
    let session_id = create_fault_session(&store, FAULT_BASE_NOW_MS);
    let execution_id = seed_fault_action(&store, &session_id, "fault-provider-timeout");

    host.supervisor_tick(DEFAULT_QUEUE_LIMIT)
        .expect("supervisor tick");
    wait_for_executor_calls(&executor, 1);
    let execution = wait_for_execution(
        &store,
        &execution_id,
        "可重试超时回到队列并写入退避",
        |execution| execution.status == ExecutionStatus::Queued && execution.retry_at_ms.is_some(),
    );
    assert!(
        execution.lease.is_none(),
        "重排后的 Execution 不得保留 lease"
    );
    assert!(
        execution
            .retry_at_ms
            .is_some_and(|retry_at| retry_at > FAULT_BASE_NOW_MS),
        "可重试失败必须写入晚于当前时刻的退避时间"
    );
    assert_eq!(
        transient_executions(&store, clock.now_ms()),
        0,
        "重排后不得残留 running/claimed"
    );

    clock.advance_by(FAULT_RETRY_ADVANCE_MS);
    host.supervisor_tick(DEFAULT_QUEUE_LIMIT)
        .expect("退避后的 supervisor tick");
    wait_for_executor_calls(&executor, 2);
    let execution = wait_for_execution(
        &store,
        &execution_id,
        "重试后进入结论态",
        |execution| execution.status.is_conclusive(),
    );

    assert_eq!(
        execution.status,
        ExecutionStatus::Completed,
        "第二次尝试应完成 Execution"
    );
    assert!(execution.lease.is_none(), "结论态不得保留 lease");
    assert_eq!(executor.calls(), 2, "退避期间不得重复执行");
    assert_eq!(
        transient_executions(&store, clock.now_ms()),
        0,
        "故障恢复后不得残留 running/claimed"
    );
}

#[test]
fn sqlite_busy_contention_settles_claim_and_releases_lease() {
    let fixture = busy_fixture("fault-busy-contention");
    let store = fixture.store.clone();
    let execution_id = fixture.execution_id.clone();
    let blocker = rusqlite::Connection::open(&fixture.path).expect("阻塞连接");
    blocker.execute_batch("BEGIN IMMEDIATE").expect("持有写锁");
    let claim_store = store.clone();
    let claim = std::thread::spawn(move || {
        claim_store.claim_executions(1, FAULT_WORKER_OWNER, FAULT_BASE_NOW_MS, FAULT_LEASE_MS)
    });
    std::thread::sleep(FAULT_BLOCKED_WINDOW);
    blocker.execute_batch("COMMIT").expect("释放写锁");

    let claimed = claim
        .join()
        .expect("领取线程不得 panic")
        .expect("写锁释放后必须完成领取");
    assert_eq!(claimed.len(), 1, "写锁释放后必须领取到排队 Execution");
    let execution = load_execution(&store, &execution_id);
    assert_eq!(execution.status, ExecutionStatus::Running);
    let lease = execution.lease.expect("领取必须建立真实 lease");
    assert_eq!(lease.owner, FAULT_WORKER_OWNER);
    assert_eq!(
        lease.expires_at_ms,
        FAULT_BASE_NOW_MS.saturating_add(FAULT_LEASE_MS),
        "busy 竞争不得篡改 lease 到期时间"
    );

    store
        .settle_execution(
            &execution_id,
            &lease.owner,
            &lease.token,
            ExecutionStatus::Completed,
            FAULT_BASE_NOW_MS,
        )
        .expect("结算 Execution");
    assert_eq!(
        transient_executions(&store, FAULT_BASE_NOW_MS),
        0,
        "结算后不得残留 running/claimed"
    );
}

#[test]
fn sqlite_busy_beyond_timeout_fails_without_phantom_claim() {
    let fixture = busy_fixture("fault-busy-timeout");
    let store = SqliteRuntimeStore::with_busy_timeout(fixture.path.clone(), FAULT_BUSY_TIMEOUT);
    let execution_id = fixture.execution_id.clone();
    let blocker = rusqlite::Connection::open(&fixture.path).expect("阻塞连接");
    blocker.execute_batch("BEGIN IMMEDIATE").expect("持有写锁");

    let started = Instant::now();
    let failure = store
        .claim_executions(1, FAULT_WORKER_OWNER, FAULT_BASE_NOW_MS, FAULT_LEASE_MS)
        .expect_err("写锁长期占用时领取必须显式失败，而不是无限阻塞");
    let elapsed = started.elapsed();
    assert!(
        elapsed < FAULT_BUSY_FAILURE_BOUND,
        "busy 失败必须有界，实际耗时 {elapsed:?}：{failure}"
    );

    let execution = load_execution(&store, &execution_id);
    assert_eq!(
        execution.status,
        ExecutionStatus::Queued,
        "失败的领取不得留下 running"
    );
    assert!(execution.lease.is_none(), "失败的领取不得留下幽灵 lease");

    blocker.execute_batch("COMMIT").expect("释放写锁");
    let claimed = store
        .claim_executions(1, FAULT_WORKER_OWNER, FAULT_BASE_NOW_MS, FAULT_LEASE_MS)
        .expect("写锁释放后必须恢复领取");
    assert_eq!(claimed.len(), 1, "写锁释放后执行必须仍可被领取");
}

/// 「一个排队 Execution + 已存在库文件」的 busy 注入夹具。
struct BusyFixture {
    store: SqliteRuntimeStore,
    path: std::path::PathBuf,
    execution_id: ExecutionId,
    // 字段顺序即析构顺序：临时目录必须最后析构，`store` 先释放 SQLite 句柄，目录才删得掉。
    _directory: axon_test_support::TestTempDir,
}

fn busy_fixture(prefix: &str) -> BusyFixture {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(path.clone());
    let session_id = create_fault_session(&store, FAULT_BASE_NOW_MS);
    let execution_id = seed_queued_execution(&store, &session_id, prefix);
    BusyFixture {
        store,
        path,
        execution_id,
        _directory: directory,
    }
}

fn fault_host(
    store: SqliteRuntimeStore,
    executor: TimeoutOnceExecutor,
    clock: FaultClock,
) -> DaemonHost<SqliteRuntimeStore, TimeoutOnceExecutor, FaultPublisher, FaultClock, FaultProcess> {
    DaemonHost::new(
        store,
        executor,
        FaultPublisher,
        clock,
        FaultProcess,
        RuntimeHostId::new("daemon-fault-host").expect("Host ID"),
        FAULT_LEASE_MS,
        FAULT_LEASE_MS,
    )
}

fn create_fault_session(store: &SqliteRuntimeStore, now_ms: u64) -> DaemonSessionId {
    let session_id = DaemonSessionId::new("fault-session").expect("Session ID");
    store
        .create_session(&session_id, ".", now_ms)
        .expect("创建 Session");
    session_id
}

/// 写入一个排队 Action；Worker 领取后以 Action ID 作为 Execution ID 结算。
fn seed_fault_action(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    action_id: &str,
) -> ExecutionId {
    let mut task = GoalRecord::new(
        GoalId::new(format!("{action_id}-task")).expect("Goal ID"),
        ScopeRef::global(),
        "故障注入任务",
        GoalPlan {
            objective: "验证故障注入后的收敛".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        FAULT_BASE_NOW_MS,
    )
    .expect("Task");
    task.transition(GoalStatus::Active, FAULT_BASE_NOW_MS)
        .expect("激活 Task");
    store.create_goal(&task).expect("写入 Task");
    // Ledger gate 要求 Task 作用域存在有效 Decision，否则 Action 会被判定为不可执行。
    store
        .append_decision(
            &DecisionRecord {
                decision_id: DecisionId::new(format!("{action_id}-allow")).expect("Decision ID"),
                scope: ScopeRef::global(),
                actor: "fault-injection".to_owned(),
                effect: DecisionEffect::Allow,
                revision: 1,
                expires_at_ms: None,
                created_at_ms: FAULT_BASE_NOW_MS,
            },
            0,
        )
        .expect("写入 Allow Decision");
    store
        .enqueue_action(
            &SessionActionRecord {
                action_id: SessionActionId::new(action_id).expect("Action ID"),
                goal_id: Some(task.goal_id),
                session_id: session_id.clone(),
                kind: "user_input".to_owned(),
                priority: 0,
                status: SessionActionStatus::Queued,
                dedupe_key: None,
                payload_json: serde_json::json!({"prompt": "故障注入"}).to_string(),
                goal_revision: None,
                context_version: None,
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            FAULT_BASE_NOW_MS,
        )
        .expect("入队 Action");
    ExecutionId::new(action_id).expect("Execution ID")
}

/// 直接写入排队 Execution；用于在不启动 Worker 的前提下注入 SQLite busy。
fn seed_queued_execution(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    execution_id: &str,
) -> ExecutionId {
    let execution_id = ExecutionId::new(execution_id).expect("Execution ID");
    let mut task = GoalRecord::new(
        GoalId::new(format!("{}-task", execution_id.as_str())).expect("Goal ID"),
        ScopeRef::global(),
        "busy 注入任务",
        GoalPlan {
            objective: "验证写锁竞争下的领取语义".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        FAULT_BASE_NOW_MS,
    )
    .expect("Task");
    task.transition(GoalStatus::Active, FAULT_BASE_NOW_MS)
        .expect("激活 Task");
    store.create_goal(&task).expect("写入 Task");
    store
        .enqueue_execution(
            &ExecutionRecord::new(execution_id.clone(), task.goal_id, FAULT_BASE_NOW_MS)
                .with_session_id(session_id.clone()),
        )
        .expect("入队 Execution");
    execution_id
}

fn load_execution(store: &SqliteRuntimeStore, execution_id: &ExecutionId) -> ExecutionRecord {
    store
        .get_execution(execution_id)
        .expect("读取 Execution")
        .expect("Execution")
}

fn wait_for_execution(
    store: &SqliteRuntimeStore,
    execution_id: &ExecutionId,
    expectation: &str,
    predicate: impl Fn(&ExecutionRecord) -> bool,
) -> ExecutionRecord {
    let deadline = Instant::now() + FAULT_QUIESCE_TIMEOUT;
    loop {
        if let Some(execution) = store.get_execution(execution_id).expect("读取 Execution")
            && predicate(&execution)
        {
            return execution;
        }
        assert!(
            Instant::now() < deadline,
            "等待 Execution 收敛超时：{expectation}"
        );
        std::thread::sleep(FAULT_POLL_INTERVAL);
    }
}

fn wait_for_executor_calls(executor: &TimeoutOnceExecutor, expected: u32) {
    let deadline = Instant::now() + FAULT_QUIESCE_TIMEOUT;
    while executor.calls() < expected {
        assert!(
            Instant::now() < deadline,
            "等待第 {expected} 次执行超时：当前 {}",
            executor.calls()
        );
        std::thread::sleep(FAULT_POLL_INTERVAL);
    }
}

/// 当前处于瞬时状态的 Execution 数量；故障收敛后必须为 0。
fn transient_executions(store: &SqliteRuntimeStore, now_ms: u64) -> u64 {
    let health = store.read_runtime_health(now_ms).expect("健康快照");
    TRANSIENT_EXECUTION_STATUSES
        .iter()
        .filter_map(|status| health.executions.by_status.get(*status))
        .fold(0_u64, |total, count| total.saturating_add(*count))
}
