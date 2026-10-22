//! 守护进程容量审计报告（R7-06）。
//!
//! 默认不参与常规测试；由 `tools/verify-jarvis-capacity.ps1` 显式触发。
//! 报告使用真实 SQLite 事实源与真实 DaemonHost，只把 Provider 执行替换为本地执行器，
//! 因此能量化 backlog 领取/结算延迟、并发入口争用、吞吐、常驻内存、WAL 体积与最老积压时长。

use super::*;
use axon_domain::{
    ConsumerCursor, GoalPlan, GoalRecord, GoalStatus, JarvisEvent, JarvisEventId, ScopeRef,
    SessionActionId, SessionActionStatus,
};
use axon_session::{
    CheckpointTelemetry, GoalStore, HealthStore, LedgerEventStore, SessionActionRecord,
    SessionEventRecord, SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};
use std::{
    path::Path,
    time::{Duration, Instant},
};

const CAPACITY_BACKLOG_SIZES: [usize; 2] = [1_000, 10_000];
const CAPACITY_BACKLOG_ENV: &str = "JARVIS_CAPACITY_BACKLOGS";
const CAPACITY_NOW_MS: u64 = 1_000_000;
/// 并发入口线程数；覆盖多个入口同时写入同一事实源的写锁争用。
const CAPACITY_CONCURRENT_THREADS: usize = 8;
/// 并发入口动作总数，按线程均分。
const CAPACITY_CONCURRENT_ACTIONS: usize = 1_000;
/// 未完成 Action 读取上限；用于断言入口动作已被全部消费。
const CAPACITY_PENDING_LIMIT: u32 = 16;
/// 事实源文件名；报告据此直接测量数据库与 WAL 体积。
const CAPACITY_DB_FILE_NAME: &str = "runtime.sqlite3";
/// WAL 文件名后缀。
const CAPACITY_WAL_SUFFIX: &str = "-wal";
const CROSS_PROCESS_DB_ENV: &str = "JARVIS_CROSS_PROCESS_DB";
const CROSS_PROCESS_CHILD_ENV: &str = "JARVIS_CROSS_PROCESS_CHILD";
const CROSS_PROCESS_EVENT_COUNT_ENV: &str = "JARVIS_CROSS_PROCESS_EVENT_COUNT";
const CROSS_PROCESS_CHILDREN: usize = 2;
const CROSS_PROCESS_EVENTS_PER_CHILD: usize = 64;
/// 内存采样单位换算：`/proc` 与 `tasklist` 均以 KiB 报告。
#[cfg(any(target_os = "linux", windows))]
const BYTES_PER_KIB: u64 = 1024;
#[cfg(target_os = "linux")]
const LINUX_PROC_STATUS_PATH: &str = "/proc/self/status";
/// `/proc/self/status` 中的峰值 RSS 字段。
#[cfg(target_os = "linux")]
const LINUX_PROC_PEAK_RSS_KEY: &str = "VmHWM:";
#[cfg(windows)]
const WINDOWS_TASKLIST_PROGRAM: &str = "tasklist";
/// `tasklist` CSV 列序号：镜像名、PID、会话名、会话号、内存占用。
#[cfg(windows)]
const WINDOWS_TASKLIST_MEMORY_COLUMN: usize = 4;

#[derive(Clone, Copy)]
struct CapacityExecutor;

impl TurnExecutor for CapacityExecutor {
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
struct CapacityPublisher;

impl EventPublisher for CapacityPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct CapacityClock;

impl RuntimeClock for CapacityClock {
    fn now_ms(&self) -> u64 {
        CAPACITY_NOW_MS
    }
}

#[derive(Clone, Copy)]
struct CapacityProcess;

impl ProcessIdentity for CapacityProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[test]
#[ignore = "容量审计报告，由 tools/verify-jarvis-capacity.ps1 触发"]
fn capacity_report_for_backlog_sizes() {
    for backlog in requested_backlogs() {
        println!("{}", run_capacity_case(backlog));
    }
}

#[test]
#[ignore = "容量审计报告，由 tools/verify-jarvis-capacity.ps1 触发"]
fn capacity_report_for_concurrent_ingress() {
    println!("{}", run_concurrent_ingress_case());
}

#[test]
#[ignore = "跨进程 SQLite 容量审计，由 R9 验收脚本显式触发"]
fn cross_process_ledger_append_is_serialized() {
    if std::env::var_os(CROSS_PROCESS_CHILD_ENV).is_some() {
        let path = std::env::var(CROSS_PROCESS_DB_ENV).expect("子进程数据库路径");
        let count = std::env::var(CROSS_PROCESS_EVENT_COUNT_ENV)
            .expect("子进程事件数量")
            .parse::<usize>()
            .expect("事件数量应为整数");
        let store = SqliteRuntimeStore::new(path);
        for index in 0..count {
            let event_key = format!("cross-process:{}:{index}", std::process::id());
            store
                .append_ledger_event(&JarvisEvent {
                    event_id: JarvisEventId::new(format!("event-{event_key}")).expect("Event ID"),
                    sequence: 0,
                    event_type: "r9.capacity.probe".to_owned(),
                    idempotency_key: event_key,
                    cause_event_id: None,
                    payload_json: "{}".to_owned(),
                    occurred_at_ms: CAPACITY_NOW_MS,
                })
                .expect("跨进程 Event 追加");
        }
        return;
    }

    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join(CAPACITY_DB_FILE_NAME);
    let store = SqliteRuntimeStore::new(database_path.clone());
    store.initialize().expect("初始化共享数据库");
    let executable = std::env::current_exe().expect("测试进程路径");
    let mut children = Vec::with_capacity(CROSS_PROCESS_CHILDREN);
    for _ in 0..CROSS_PROCESS_CHILDREN {
        children.push(
            std::process::Command::new(&executable)
                .arg("application::daemon_host::daemon_host_capacity_tests::cross_process_ledger_append_is_serialized")
                .arg("--exact")
                .arg("--ignored")
                .arg("--nocapture")
                .env(CROSS_PROCESS_CHILD_ENV, "1")
                .env(CROSS_PROCESS_DB_ENV, &database_path)
                .env(
                    CROSS_PROCESS_EVENT_COUNT_ENV,
                    CROSS_PROCESS_EVENTS_PER_CHILD.to_string(),
                )
                .spawn()
                .expect("启动跨进程子测试"),
        );
    }
    for mut child in children {
        let status = child.wait().expect("等待跨进程子测试");
        assert!(status.success(), "跨进程子测试失败：{status}");
    }

    let events = store
        .replay_ledger_events(
            0,
            (CROSS_PROCESS_CHILDREN * CROSS_PROCESS_EVENTS_PER_CHILD) as u32,
        )
        .expect("读取跨进程 Event");
    assert_eq!(
        events.len(),
        CROSS_PROCESS_CHILDREN * CROSS_PROCESS_EVENTS_PER_CHILD,
        "跨进程追加不得丢失 Event"
    );
    assert!(
        events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
}

/// 读取待测 backlog 规模；默认覆盖 1k/10k。
fn requested_backlogs() -> Vec<usize> {
    let Ok(requested) = std::env::var(CAPACITY_BACKLOG_ENV) else {
        return CAPACITY_BACKLOG_SIZES.to_vec();
    };
    let sizes = requested
        .split(',')
        .filter_map(|value| value.trim().parse::<usize>().ok())
        .filter(|size| *size > 0)
        .collect::<Vec<_>>();
    if sizes.is_empty() {
        CAPACITY_BACKLOG_SIZES.to_vec()
    } else {
        sizes
    }
}

fn run_capacity_case(backlog: usize) -> String {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join(CAPACITY_DB_FILE_NAME));
    let scope = register_capacity_workspace(&store, directory.path());
    let context = seed_backlog(&store, backlog, &scope, directory.path());
    let host = test_host(store.clone());
    let oldest_before = store
        .read_runtime_health(CAPACITY_NOW_MS)
        .expect("健康快照")
        .execution_backlog
        .oldest_age_ms
        .unwrap_or(0);
    let rss_before = resident_memory_bytes();

    let started = Instant::now();
    let mut latencies = Vec::with_capacity(context.session_ids.len());
    for session_id in &context.session_ids {
        let action_started = Instant::now();
        host.pump_session_once(session_id.clone())
            .expect("处理单个 Session")
            .expect("应处理排队的 Action");
        latencies.push(action_started.elapsed());
    }
    let total = started.elapsed();

    let health = store
        .read_runtime_health(CAPACITY_NOW_MS)
        .expect("健康快照");
    let rss_after = resident_memory_bytes();
    let wal_bytes = file_len(
        &directory
            .path()
            .join(format!("{CAPACITY_DB_FILE_NAME}{CAPACITY_WAL_SUFFIX}")),
    );
    let db_bytes = file_len(&directory.path().join(CAPACITY_DB_FILE_NAME));
    serde_json::json!({
        "case": "backlog",
        "backlog": backlog,
        "drained": latencies.len(),
        "p50_ms": percentile_ms(&mut latencies, 50.0),
        "p95_ms": percentile_ms(&mut latencies, 95.0),
        "total_ms": total.as_millis(),
        "oldest_backlog_age_ms_before": oldest_before,
        "oldest_backlog_age_ms_after": health.execution_backlog.oldest_age_ms.unwrap_or(0),
        "rss_before_bytes": rss_before,
        "rss_after_bytes": rss_after,
        "wal_bytes": wal_bytes,
        "db_bytes": db_bytes,
    })
    .to_string()
}

/// 并发入口容量：多线程同时写入同一事实源，必须全部提交成功且无丢失。
fn run_concurrent_ingress_case() -> String {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join(CAPACITY_DB_FILE_NAME));
    store.initialize().expect("初始化事实源");
    let scope = register_capacity_workspace(&store, directory.path());
    let per_thread = CAPACITY_CONCURRENT_ACTIONS / CAPACITY_CONCURRENT_THREADS;
    let rss_before = resident_memory_bytes();

    let started = Instant::now();
    let mut handles = Vec::with_capacity(CAPACITY_CONCURRENT_THREADS);
    for thread in 0..CAPACITY_CONCURRENT_THREADS {
        let store = store.clone();
        let scope = scope.clone();
        handles.push(std::thread::spawn(move || {
            seed_concurrent_batch(&store, &scope, thread, per_thread)
        }));
    }
    let mut ingress_latencies = Vec::with_capacity(CAPACITY_CONCURRENT_ACTIONS);
    let mut session_ids = Vec::with_capacity(CAPACITY_CONCURRENT_ACTIONS);
    let mut failures = Vec::new();
    for handle in handles {
        let batch = handle.join().expect("并发入口线程");
        ingress_latencies.extend(batch.latencies);
        session_ids.extend(batch.session_ids);
        failures.extend(batch.failures);
    }
    let ingress_total = started.elapsed();

    assert!(failures.is_empty(), "并发入口出现写入失败: {failures:?}");
    assert_eq!(
        ingress_latencies.len(),
        CAPACITY_CONCURRENT_ACTIONS,
        "并发入口动作数与预期不符"
    );
    let tasks = store
        .read_runtime_health(CAPACITY_NOW_MS)
        .expect("健康快照")
        .goals
        .total;
    assert_eq!(
        tasks, CAPACITY_CONCURRENT_ACTIONS as u64,
        "并发入口写入的 Task 数量与预期不符"
    );

    let host = test_host(store.clone());
    let drain_started = Instant::now();
    for session_id in &session_ids {
        host.pump_session_once(session_id.clone())
            .expect("处理单个 Session")
            .expect("应处理排队的 Action");
    }
    let drain_total = drain_started.elapsed();
    for session_id in &session_ids {
        let pending = store
            .list_pending_actions(session_id, CAPACITY_PENDING_LIMIT)
            .expect("未完成 Action");
        assert!(pending.is_empty(), "并发入口后仍有未消费 Action");
    }
    let rss_after = resident_memory_bytes();
    let wal_bytes = file_len(
        &directory
            .path()
            .join(format!("{CAPACITY_DB_FILE_NAME}{CAPACITY_WAL_SUFFIX}")),
    );
    let db_bytes = file_len(&directory.path().join(CAPACITY_DB_FILE_NAME));
    serde_json::json!({
        "case": "concurrent_ingress",
        "threads": CAPACITY_CONCURRENT_THREADS,
        "actions": CAPACITY_CONCURRENT_ACTIONS,
        "ingress_p50_ms": percentile_ms(&mut ingress_latencies, 50.0),
        "ingress_p95_ms": percentile_ms(&mut ingress_latencies, 95.0),
        "ingress_total_ms": ingress_total.as_millis(),
        "drain_total_ms": drain_total.as_millis(),
        "enqueue_failures": failures.len(),
        "rss_before_bytes": rss_before,
        "rss_after_bytes": rss_after,
        "wal_bytes": wal_bytes,
        "db_bytes": db_bytes,
    })
    .to_string()
}

struct CapacityContext {
    session_ids: Vec<DaemonSessionId>,
}

/// 一个并发入口线程提交的批次：延迟、Session 与失败信息。
struct CapacityIngressBatch {
    latencies: Vec<Duration>,
    session_ids: Vec<DaemonSessionId>,
    failures: Vec<String>,
}

/// 并发入口线程体：各自独立创建 Session/Task 并入队 Action。
fn seed_concurrent_batch(
    store: &SqliteRuntimeStore,
    scope: &ScopeRef,
    thread: usize,
    actions: usize,
) -> CapacityIngressBatch {
    let mut latencies = Vec::with_capacity(actions);
    let mut session_ids = Vec::with_capacity(actions);
    let mut failures = Vec::new();
    for index in 0..actions {
        let session_id =
            DaemonSessionId::new(format!("capacity-concurrent-{thread}-{index}")).expect("Session");
        store.create_session(&session_id, ".", 10).expect("Session");
        let action_started = Instant::now();
        match enqueue_seeded_action(store, scope, &session_id, &format!("{thread}-{index}")) {
            Ok(()) => latencies.push(action_started.elapsed()),
            Err(error) => failures.push(error.to_string()),
        }
        session_ids.push(session_id);
    }
    CapacityIngressBatch {
        latencies,
        session_ids,
        failures,
    }
}

fn seed_backlog(
    store: &SqliteRuntimeStore,
    backlog: usize,
    scope: &ScopeRef,
    cwd: &std::path::Path,
) -> CapacityContext {
    let mut session_ids = Vec::with_capacity(backlog);
    for index in 0..backlog {
        let session_id =
            DaemonSessionId::new(format!("capacity-session-{index}")).expect("Session");
        store
            .create_session(&session_id, &cwd.to_string_lossy(), 10)
            .expect("Session");
        enqueue_seeded_action(store, scope, &session_id, &index.to_string()).expect("Action 入队");
        session_ids.push(session_id);
    }
    CapacityContext { session_ids }
}

/// 为一个 Session 写入容量审计用 Task 与已排队 Action。
fn enqueue_seeded_action(
    store: &SqliteRuntimeStore,
    scope: &ScopeRef,
    session_id: &DaemonSessionId,
    suffix: &str,
) -> Result<(), axon_session::RuntimeStoreError> {
    let mut goal = GoalRecord::new(
        axon_domain::GoalId::new(format!("capacity-goal-{suffix}")).expect("Task"),
        scope.clone(),
        format!("容量任务 {suffix}"),
        GoalPlan {
            objective: "验证 backlog 领取与结算容量".to_owned(),
            steps: Vec::new(),
        },
        Vec::new(),
        10,
    )
    .expect("Task");
    goal.transition(GoalStatus::Active, 11).expect("Task 激活");
    store.create_goal(&goal).expect("Task 写入");
    store.enqueue_action(
        &SessionActionRecord {
            action_id: SessionActionId::new(format!("capacity-action-{suffix}")).expect("Action"),
            goal_id: Some(goal.goal_id.clone()),
            session_id: session_id.clone(),
            kind: "user_input".to_owned(),
            priority: 0,
            status: SessionActionStatus::Queued,
            dedupe_key: None,
            payload_json: format!(r#"{{"prompt":"容量审计 {suffix}"}}"#),
            goal_revision: None,
            context_version: None,
            claimed_by: None,
            claimed_generation: None,
            lease_expires_at_ms: None,
            attempt: 0,
        },
        10,
    )?;
    Ok(())
}

fn register_capacity_workspace(store: &SqliteRuntimeStore, path: &std::path::Path) -> ScopeRef {
    let workspace = WorkspaceResolver
        .resolve(path, CAPACITY_NOW_MS)
        .expect("解析容量 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册容量 Workspace");
    ScopeRef::workspace(&workspace.workspace_id).expect("构造容量作用域")
}

fn test_host(
    store: SqliteRuntimeStore,
) -> DaemonHost<
    SqliteRuntimeStore,
    CapacityExecutor,
    CapacityPublisher,
    CapacityClock,
    CapacityProcess,
> {
    let mut host = DaemonHost::new(
        store,
        CapacityExecutor,
        CapacityPublisher,
        CapacityClock,
        CapacityProcess,
        RuntimeHostId::new("daemon-capacity-host").expect("Host ID"),
        60_000,
        60_000,
    );
    host.start().expect("启动 Host");
    host
}

/// 采样当前进程常驻内存字节数。
///
/// Linux 读取 `/proc/self/status` 的峰值 RSS（`VmHWM`）；Windows 读取 `tasklist`
/// 报告的当前工作集。采样失败或平台不支持时返回 `None`，报告输出 `null`，
/// 不用零值冒充真实测量。
fn resident_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(LINUX_PROC_STATUS_PATH).ok()?;
        let line = status
            .lines()
            .find(|line| line.starts_with(LINUX_PROC_PEAK_RSS_KEY))?;
        let kilobytes = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
        kilobytes.checked_mul(BYTES_PER_KIB)
    }
    #[cfg(windows)]
    {
        let pid_filter = format!("PID eq {}", std::process::id());
        let output = std::process::Command::new(WINDOWS_TASKLIST_PROGRAM)
            .args(["/FI", &pid_filter, "/FO", "CSV", "/NH"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let column = text
            .lines()
            .next()?
            .split("\",\"")
            .nth(WINDOWS_TASKLIST_MEMORY_COLUMN)?;
        let kilobytes = column
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>()
            .parse::<u64>()
            .ok()?;
        kilobytes.checked_mul(BYTES_PER_KIB)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        None
    }
}

fn percentile_ms(latencies: &mut [Duration], percentile: f64) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }
    latencies.sort_unstable();
    let rank = ((percentile / 100.0) * latencies.len() as f64).ceil() as usize;
    let index = rank.saturating_sub(1).min(latencies.len() - 1);
    latencies[index].as_secs_f64() * 1_000.0
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}
