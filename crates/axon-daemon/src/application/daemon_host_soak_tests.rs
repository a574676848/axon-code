//! JARVIS 常驻 soak 报告（R7-07）。
//!
//! 默认不参与常规测试；由 `tools/verify-jarvis-soak.ps1` 显式触发。
//! 每个周期覆盖 Host heartbeat、lease 续租、recovery tick 与 Execution 领取/结算，
//! 并断言常驻状态（Session Worker 登记、Worker 线程数、非终态 Execution、WAL 体积）
//! 不随周期数增长。

use super::*;
use axon_domain::{
    ConsumerCursor, DecisionEffect, DecisionId, DecisionRecord, GoalPlan, GoalRecord, GoalStatus,
    HealthStatusCounts, ScopeRef, SessionActionId, SessionActionStatus,
};
use axon_session::{
    CheckpointTelemetry, DecisionStore, GoalStore, HealthStore, SessionActionRecord,
    SessionEventRecord, SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};
use std::{
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

const SOAK_ITERATIONS_ENV: &str = "JARVIS_SOAK_ITERATIONS";
const DEFAULT_SOAK_ITERATIONS: usize = 1_000;
const SOAK_SESSION_COUNT: usize = 4;
const SOAK_BASE_NOW_MS: u64 = 5_000_000;
const SOAK_CYCLE_MS: u64 = 1_000;
const SOAK_HOST_LEASE_MS: u64 = 3_000;
const SOAK_ACTION_LEASE_MS: u64 = 2_000;
const SOAK_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);
const SOAK_DRAIN_POLL_INTERVAL: Duration = Duration::from_millis(1);
const SOAK_MAX_WAL_BYTES: u64 = 64 * 1024 * 1024;
const WAL_FILE_NAME: &str = "runtime.sqlite3-wal";
const NON_TERMINAL_EXECUTION_STATUSES: [&str; 4] = ["queued", "claimed", "running", "retrying"];

#[derive(Clone, Copy)]
struct SoakExecutor;

impl TurnExecutor for SoakExecutor {
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
struct SoakPublisher;

impl EventPublisher for SoakPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone)]
struct SoakClock(Arc<AtomicU64>);

impl SoakClock {
    fn advance_to(&self, now_ms: u64) {
        self.0.store(now_ms, Ordering::Release);
    }
}

impl RuntimeClock for SoakClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy)]
struct SoakProcess;

impl ProcessIdentity for SoakProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[test]
#[ignore = "常驻 soak 报告，由 tools/verify-jarvis-soak.ps1 触发"]
fn soak_report_for_heartbeat_and_execution_churn() {
    println!("{}", run_soak_case(soak_iterations()));
}

/// 读取 soak 周期数；默认 1000 轮。
fn soak_iterations() -> usize {
    std::env::var(SOAK_ITERATIONS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SOAK_ITERATIONS)
}

fn run_soak_case(iterations: usize) -> String {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let clock = SoakClock(Arc::new(AtomicU64::new(SOAK_BASE_NOW_MS)));
    let mut host = DaemonHost::new(
        store.clone(),
        SoakExecutor,
        SoakPublisher,
        clock.clone(),
        SoakProcess,
        RuntimeHostId::new("daemon-soak-host").expect("Host ID"),
        SOAK_HOST_LEASE_MS,
        SOAK_ACTION_LEASE_MS,
    );
    host.start().expect("启动 Host");
    let workspace = WorkspaceResolver
        .resolve(directory.path(), SOAK_BASE_NOW_MS)
        .expect("解析 soak Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 soak Workspace");
    let scope = ScopeRef::workspace(&workspace.workspace_id).expect("构造 soak 作用域");
    seed_allow_decision(&store, &scope);
    let sessions = seed_sessions(&store, &clock, directory.path());
    let started = Instant::now();
    let mut peak_active_sessions = 0_usize;
    let mut peak_pending_sessions = 0_usize;
    for iteration in 0..iterations {
        let now_ms = SOAK_BASE_NOW_MS.saturating_add(iteration as u64 * SOAK_CYCLE_MS);
        clock.advance_to(now_ms);
        // heartbeat：每轮续租，跨越多轮 Host lease 窗口。
        host.renew().expect("Host 续租");
        let session_id = sessions[iteration % sessions.len()].clone();
        seed_execution(&store, &session_id, &scope, iteration, now_ms);
        host.supervisor_tick(DEFAULT_QUEUE_LIMIT)
            .expect("supervisor tick");
        drain_until_idle(&store, &clock, now_ms);
        peak_active_sessions = peak_active_sessions.max(host.worker_registry.active_sessions());
        peak_pending_sessions = peak_pending_sessions.max(host.worker_registry.pending_sessions());
    }
    let health = store.read_runtime_health(clock.now_ms()).expect("健康快照");
    let snapshot = host.health_snapshot().expect("宿主健康快照");
    let stuck = non_terminal_executions(&health.executions);
    assert_eq!(stuck, 0, "soak 结束后仍有非终态 Execution");
    // 结算完成与 Worker 登记释放之间存在收尾时序：等到登记真正清空再断言，
    // 避免把正常的退出时序误判为泄漏。
    let registry_deadline = Instant::now() + SOAK_DRAIN_TIMEOUT;
    while host.worker_registry.tracked_sessions() > 0 {
        assert!(
            Instant::now() < registry_deadline,
            "soak 结束后仍登记着 Session Worker"
        );
        std::thread::sleep(SOAK_DRAIN_POLL_INTERVAL);
    }
    assert!(
        peak_active_sessions <= SOAK_SESSION_COUNT,
        "Session Worker 活跃登记峰值 {peak_active_sessions} 超出并发上限"
    );
    assert!(
        peak_pending_sessions <= SOAK_SESSION_COUNT,
        "Session Worker 待唤醒登记峰值 {peak_pending_sessions} 超出有界上限"
    );
    let workers = host
        .execution_worker_pool
        .get_or_create_with_capacity(
            EXECUTION_WORKER_MAX_CONCURRENCY,
            EXECUTION_WORKER_QUEUE_CAPACITY,
        )
        .expect("执行 Worker 池")
        .worker_count();
    assert!(
        workers <= EXECUTION_WORKER_MAX_CONCURRENCY,
        "执行 Worker 线程数 {workers} 超出上限"
    );
    let wal_bytes = file_len(&directory.path().join(WAL_FILE_NAME));
    assert!(
        wal_bytes <= SOAK_MAX_WAL_BYTES,
        "WAL 体积 {wal_bytes} 字节超出上限"
    );
    serde_json::json!({
        "iterations": iterations,
        "sessions": SOAK_SESSION_COUNT,
        "duration_ms": started.elapsed().as_millis(),
        "peak_active_sessions": peak_active_sessions,
        "peak_pending_sessions": peak_pending_sessions,
        "execution_worker_threads": workers,
        "terminal_executions": health.executions.total,
        "execution_statuses": health.executions.by_status,
        "stuck_executions": stuck,
        "host_lease_remaining_ms": snapshot.host.lease_remaining_ms,
        "wal_bytes": wal_bytes,
    })
    .to_string()
}

/// 推进到没有非终态 Execution 为止；超时是 soak 失败而不是放弃等待。
fn drain_until_idle(store: &SqliteRuntimeStore, clock: &SoakClock, now_ms: u64) {
    let deadline = Instant::now() + SOAK_DRAIN_TIMEOUT;
    loop {
        let health = store.read_runtime_health(now_ms).expect("健康快照");
        if non_terminal_executions(&health.executions) == 0 && health.execution_backlog.pending == 0
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "等待 Execution 结算超时：{}",
            clock.now_ms()
        );
        std::thread::sleep(SOAK_DRAIN_POLL_INTERVAL);
    }
}

fn non_terminal_executions(counts: &HealthStatusCounts) -> u64 {
    NON_TERMINAL_EXECUTION_STATUSES
        .iter()
        .filter_map(|status| counts.by_status.get(*status))
        .fold(0_u64, |total, count| total.saturating_add(*count))
}

/// soak 任务使用已注册 Workspace 作用域；缺少有效 Decision 时 Ledger gate 会直接取消 Action，
/// 导致周期只覆盖「领取-取消」而无法覆盖真实的执行与结算。
fn seed_allow_decision(store: &SqliteRuntimeStore, scope: &ScopeRef) {
    store
        .append_decision(
            &DecisionRecord {
                decision_id: DecisionId::new("soak-allow").expect("Decision ID"),
                scope: scope.clone(),
                actor: "soak".to_owned(),
                effect: DecisionEffect::Allow,
                revision: 1,
                expires_at_ms: None,
                created_at_ms: SOAK_BASE_NOW_MS,
            },
            0,
        )
        .expect("写入 Allow Decision");
}

fn seed_sessions(
    store: &SqliteRuntimeStore,
    clock: &SoakClock,
    cwd: &Path,
) -> Vec<DaemonSessionId> {
    (0..SOAK_SESSION_COUNT)
        .map(|index| {
            let session_id =
                DaemonSessionId::new(format!("soak-session-{index}")).expect("Session ID");
            store
                .create_session(&session_id, &cwd.to_string_lossy(), clock.now_ms())
                .expect("创建 Session");
            session_id
        })
        .collect()
}

fn seed_execution(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    scope: &ScopeRef,
    iteration: usize,
    now_ms: u64,
) {
    let mut task = GoalRecord::new(
        axon_domain::GoalId::new(format!("soak-task-{iteration}")).expect("Goal ID"),
        scope.clone(),
        format!("soak 任务 {iteration}"),
        GoalPlan {
            objective: "验证常驻 heartbeat 与执行 churn".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        now_ms,
    )
    .expect("Task");
    task.transition(GoalStatus::Active, now_ms)
        .expect("激活 Task");
    store.create_goal(&task).expect("写入 Task");
    store
        .enqueue_action(
            &SessionActionRecord {
                action_id: SessionActionId::new(format!("soak-action-{iteration}"))
                    .expect("Action"),
                goal_id: Some(task.goal_id.clone()),
                session_id: session_id.clone(),
                kind: "user_input".to_owned(),
                priority: 0,
                status: SessionActionStatus::Queued,
                dedupe_key: None,
                payload_json: format!(r#"{{"prompt":"soak 轮次 {iteration}"}}"#),
                goal_revision: None,
                context_version: None,
                claimed_by: None,
                claimed_generation: None,
                lease_expires_at_ms: None,
                attempt: 0,
            },
            now_ms,
        )
        .expect("入队 Action");
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}
