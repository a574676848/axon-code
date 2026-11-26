//! Daemon 后台 Worker 线程的循环与失败处理。

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    CRON_TICK_INTERVAL_MS, CliDaemonHost, DaemonMaintenanceSignal, HOST_RENEW_INTERVAL_MS,
    MAINTENANCE_TIMER_INTERVAL_MS, RELEASE_GC_INTERVAL_MS, SCHEDULE_LEASE_DURATION_MS,
    SUPERVISOR_BATCH_LIMIT,
};

pub(super) fn start_background_workers(
    host: &CliDaemonHost,
    cron_adapter: crate::cron_commands::CronDaemonAdapter,
    workers_running: Arc<AtomicBool>,
    host_fenced: Arc<AtomicBool>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
    primary_turn_signal: Arc<DaemonMaintenanceSignal>,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
) {
    start_lease_worker(
        host.clone(),
        Arc::clone(&workers_running),
        Arc::clone(&host_fenced),
    );
    start_primary_turn_worker(
        host.clone(),
        Arc::clone(&workers_running),
        Arc::clone(&host_fenced),
        primary_turn_signal,
        Arc::clone(&maintenance_signal),
    );
    start_supervisor_worker(
        host.clone(),
        active_primary_turns,
        Arc::clone(&workers_running),
        host_fenced,
        maintenance_signal,
    );
    /*
     * WAL 回写单独占一条线程：它要做一次真正的磁盘回写，挂在监督循环里会让每秒一次的
     * 租约续期与维护 tick 一起排到回写后面。独立之后，回写慢只影响自己那一轮。
     */
    start_checkpoint_worker(host.clone(), Arc::clone(&workers_running));
    start_cron_worker(cron_adapter, workers_running);
}

/// 两次续租重试之间的退避。
const LEASE_RENEW_RETRY_DELAY: Duration = Duration::from_millis(750);
/// 持续失败的告警节流：每约 30 秒留一条 WARNING，风暴期间不刷日志。
const LEASE_RENEW_WARN_EVERY: u32 = 40;

fn start_lease_worker(host: CliDaemonHost, running: Arc<AtomicBool>, fenced: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::Acquire) {
            if let Err(error) =
                renew_until_fenced(|| host.renew(), &running, LEASE_RENEW_RETRY_DELAY)
            {
                axon_logger::error(&format!(
                    "Daemon Host 已失去 lease，后台 Worker 将停止: {error}"
                ));
                // 只停 Worker 会留下「进程活着但没人干活」的僵尸：必须把 fence 交给进程
                // 退出判据，运行函数据此以非零码退出，服务管理器才会重启。
                fenced.store(true, Ordering::Release);
                running.store(false, Ordering::Release);
                break;
            }
            thread::sleep(Duration::from_millis(HOST_RENEW_INTERVAL_MS));
        }
    });
}

/// 续租直到成功或被 fence。
///
/// 只有 `is_fenced()` 才是终态：它表示另一个 Host 已经接管，继续运行必须立刻停。
/// 存储抖动（`database is locked` 这类）**不是**终态，必须一直按固定退避重试：
/// 上一版把重试次数钉成 8 次（约 6 秒），于是任何一次几十秒的写锁风暴都会把所有
/// 后台 Worker 永久停掉，而进程既不退出也不再干活，`daemon status` 仍报 running。
/// 用户侧表现为「每一条消息都发不出去、界面停在生成中」，只能重启 Daemon 才能恢复。
///
/// 续租与业务写共用同一个 SQLite 单写者锁：一次 checkpoint 或大事务就能让
/// `renew_host` 在 busy_timeout 之后返回 `database is locked`。抖动可重试；
/// 代际失效（另一个 Host 已接管，或租约已过期）不可重试，必须立刻 fail-closed。
fn renew_until_fenced<F>(
    mut renew: F,
    running: &AtomicBool,
    retry_delay: Duration,
) -> Result<(), axon_daemon::DaemonHostError>
where
    F: FnMut() -> Result<(), axon_daemon::DaemonHostError>,
{
    let mut attempt: u32 = 0;
    loop {
        match renew() {
            Ok(()) => {
                if attempt > 0 {
                    axon_logger::warning(&format!(
                        "Daemon Host 续租在第 {attempt} 次重试后恢复，后台 Worker 继续运行"
                    ));
                }
                return Ok(());
            }
            Err(error) if error.is_fenced() => return Err(error),
            Err(error) => {
                attempt = attempt.saturating_add(1);
                if attempt % LEASE_RENEW_WARN_EVERY == 1 {
                    axon_logger::warning(&format!(
                        "Daemon Host 续租遇到存储抖动，按 {}ms 持续重试（累计 {attempt} 次）: {error}",
                        LEASE_RENEW_RETRY_DELAY.as_millis()
                    ));
                }
                if !running.load(Ordering::Acquire) {
                    return Ok(());
                }
                thread::sleep(retry_delay);
            }
        }
    }
}

fn start_primary_turn_worker(
    host: CliDaemonHost,
    running: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
) {
    thread::spawn(move || {
        run_primary_turn_loop(host, running, fenced, signal, maintenance_signal);
    });
}

fn run_primary_turn_loop(
    host: CliDaemonHost,
    running: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
) {
    while running.load(Ordering::Acquire) {
        let result = host.primary_turn_tick(SUPERVISOR_BATCH_LIMIT);
        handle_primary_turn_failures(&result);
        let backlog_remains = result
            .as_ref()
            .is_ok_and(|report| report.claimed >= SUPERVISOR_BATCH_LIMIT);
        let completed_work = result.as_ref().is_ok_and(|report| report.claimed > 0);
        if !handle_host_tick(result, &running, &fenced, "Primary Turn") {
            break;
        }
        if completed_work {
            maintenance_signal.notify();
        }
        if !backlog_remains {
            signal.wait(Duration::from_millis(MAINTENANCE_TIMER_INTERVAL_MS));
        }
    }
}

fn start_supervisor_worker(
    host: CliDaemonHost,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
    running: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
) {
    thread::spawn(move || {
        run_supervisor_loop(host, active_primary_turns, running, fenced, signal);
    });
}

fn run_supervisor_loop(
    host: CliDaemonHost,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
    running: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
) {
    let mut last_release_gc = Instant::now();
    let mut last_lsp_sample = Instant::now();
    let mut last_lsp_active = None;
    let mut last_active_turns = None;
    while running.load(Ordering::Acquire) {
        cleanup_releases_if_due(&mut last_release_gc);
        sample_and_broadcast_lsp_if_due(
            &host,
            &active_primary_turns,
            &mut last_lsp_sample,
            &mut last_lsp_active,
            &mut last_active_turns,
        );
        if !handle_host_tick(
            host.scheduler_tick(SCHEDULE_LEASE_DURATION_MS, SUPERVISOR_BATCH_LIMIT),
            &running,
            &fenced,
            "Scheduler",
        ) || !handle_host_tick(
            host.supervisor_tick(SUPERVISOR_BATCH_LIMIT),
            &running,
            &fenced,
            "Supervisor",
        ) {
            break;
        }
        signal.wait(Duration::from_millis(MAINTENANCE_TIMER_INTERVAL_MS));
    }
}

/// Cron 任务独立推进：任务体可能长时间运行，与调度/监督 tick 共线程会让
/// Daemon 维护停顿，因此单独占用一个后台线程并保持同一 tick 频率。
fn start_cron_worker(
    cron_adapter: crate::cron_commands::CronDaemonAdapter,
    running: Arc<AtomicBool>,
) {
    thread::spawn(move || run_cron_loop(cron_adapter, running));
}

fn run_cron_loop(cron_adapter: crate::cron_commands::CronDaemonAdapter, running: Arc<AtomicBool>) {
    // 停机期间错过的触发点不补跑：只有本进程在线期间到期的任务才会执行。
    let scheduler_alive_since_ms = axon_time::unix_now_ms();
    while running.load(Ordering::Acquire) {
        run_due_cron_jobs(&cron_adapter, scheduler_alive_since_ms);
        thread::sleep(Duration::from_millis(CRON_TICK_INTERVAL_MS));
    }
}

/// WAL 回写独立线程：按体量或固定周期做一次 `PASSIVE` 回写。
fn start_checkpoint_worker(host: CliDaemonHost, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        let mut last_checkpoint_at = Instant::now();
        while running.load(Ordering::Acquire) {
            checkpoint_wal_if_due(&host, &mut last_checkpoint_at);
            thread::sleep(Duration::from_millis(CHECKPOINT_WORKER_TICK_MS));
        }
    });
}

/// 回写线程的唤醒粒度：比回写周期短得多，因此只做一次比较就回去睡。
///
/// 它同时是体量触发的检测延迟上界：WAL 越过阈值到真正回写，最多晚一个 tick。
const CHECKPOINT_WORKER_TICK_MS: u64 = 30_000;

fn cleanup_releases_if_due(last_release_gc: &mut Instant) {
    if last_release_gc.elapsed() < Duration::from_millis(RELEASE_GC_INTERVAL_MS) {
        return;
    }
    crate::upgrade_commands::cleanup_releases_after_startup();
    *last_release_gc = Instant::now();
}

const LSP_OBSERVATION_SAMPLE_INTERVAL_MS: u64 = 3_000;

/// WAL 体量触发阈值：`-wal` 文件超过这个字节数就立即回写，不等固定周期。
///
/// 取 64 MiB，与 `axon-session` 连接池里的 `journal_size_limit` 相等，比较必须写成
/// **严格大于**。一次成功的回写会重置 WAL，随后的第一次提交把文件截到
/// `journal_size_limit`，文件长度从此是「高水位」而不是当前帧量：长度恰好等于 64 MiB
/// 时，WAL 里可能一帧都没有。写成严格大于，两种情况才归到同一句话：文件长度超过阈值，
/// 等价于「自上次重置起新写入的帧超过 64 MiB」。若写成大于等于，文件被截到基线之后
/// 每一次 tick 都会判定超阈值，体量触发就退化成固定 30 秒回写。
///
/// 为什么是 64 MiB：`PASSIVE` 回写要把 WAL 帧逐页拷回主库，拷贝期间持有写锁，阈值越大
/// 单次独占越久（连接池的 `busy_timeout` 是 2 秒），阈值越小回写越频繁但每次越便宜。
/// 64 MiB 让单次回写通常落在数百毫秒内，同时把 WAL 常驻体量压在「64 MiB 加一个 tick
/// 的写入量」以内。
const WAL_SIZE_CHECKPOINT_THRESHOLD_BYTES: u64 = 64 * 1024 * 1024;

/// WAL 回写周期：体量没到阈值时的兜底。
///
/// 持续写入下 WAL 会一直漂大，漂到与主库同量级后一次全量回写要独占写锁数十秒，
/// 所有写者按 `busy_timeout` 排队失败，续租与业务写一起被拉下水。周期性地做
/// `PASSIVE` 回写把这件事摊平：不阻塞读写，拿不到干净窗口就留给下一轮。
///
/// 连接池关掉自动回写之后，周期性回写只剩这条线程：慢速累积的库等不到体量阈值，
/// 也必须有一次周期性的重置，否则 WAL 仍会按写入量一直长。
const WAL_CHECKPOINT_INTERVAL_MS: u64 = 5 * 60 * 1_000;

/// 这一轮是否该回写：体量超阈值优先，否则按固定周期兜底。
fn checkpoint_due(wal_bytes: u64, since_last_checkpoint: Duration) -> bool {
    wal_bytes > WAL_SIZE_CHECKPOINT_THRESHOLD_BYTES
        || since_last_checkpoint >= Duration::from_millis(WAL_CHECKPOINT_INTERVAL_MS)
}

/// 两个库 `-wal` 文件的字节数之和：控制面与待消费队列各有一份。
///
/// 读不到就按 0 计：文件可能还没创建，也可能刚被上一轮回写删掉，这两种情况都不该让
/// 巡检线程报错或者跳过周期兜底。
fn runtime_wal_bytes(host: &CliDaemonHost) -> u64 {
    host.store()
        .database_paths()
        .into_iter()
        .map(wal_file_bytes)
        .sum()
}

/// 数据库文件对应的 WAL 文件字节数。
fn wal_file_bytes(database_path: &Path) -> u64 {
    let mut wal_path = database_path.as_os_str().to_owned();
    wal_path.push("-wal");
    std::fs::metadata(PathBuf::from(wal_path)).map_or(0, |metadata| metadata.len())
}

fn checkpoint_wal_if_due(host: &CliDaemonHost, last_checkpoint_at: &mut Instant) {
    let wal_bytes = runtime_wal_bytes(host);
    if !checkpoint_due(wal_bytes, last_checkpoint_at.elapsed()) {
        return;
    }
    if wal_bytes > WAL_SIZE_CHECKPOINT_THRESHOLD_BYTES {
        axon_logger::debug(&format!(
            "WAL 已达 {wal_bytes} 字节，跳过回写周期等待，立即按体量回写"
        ));
    }
    *last_checkpoint_at = Instant::now();
    match host.store().checkpoint_wal() {
        Ok(checkpoint) if checkpoint.busy => axon_logger::debug(&format!(
            "WAL 回写被并发读者挡住，剩余 {} 帧留给下一轮",
            checkpoint
                .log_frames
                .saturating_sub(checkpoint.checkpointed_frames)
        )),
        Ok(checkpoint) if checkpoint.checkpointed_frames > 0 => axon_logger::debug(&format!(
            "WAL 回写完成：{} 帧已落主库",
            checkpoint.checkpointed_frames
        )),
        Ok(_) => {}
        Err(error) => axon_logger::warning(&format!("WAL 回写失败，将在下一轮重试: {error}")),
    }
}

fn sample_and_broadcast_lsp_if_due(
    host: &CliDaemonHost,
    active_primary_turns: &crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
    last_sample_at: &mut Instant,
    last_active_clients: &mut Option<usize>,
    last_active_turns: &mut Option<usize>,
) {
    if last_sample_at.elapsed() < Duration::from_millis(LSP_OBSERVATION_SAMPLE_INTERVAL_MS) {
        return;
    }
    *last_sample_at = Instant::now();
    let stats = axon_agent_runtime::LspClientPoolRuntime::stats_snapshot();
    let current_active_turns = active_primary_turns.active_count();
    if *last_active_clients == Some(stats.active_clients)
        && *last_active_turns == Some(current_active_turns)
    {
        return;
    }
    *last_active_clients = Some(stats.active_clients);
    *last_active_turns = Some(current_active_turns);
    let frame = axon_domain::RuntimeObservationFrame::new(axon_time::unix_now_ms(), "", "")
        .with_lsp(axon_domain::LspMetricsSnapshot {
            active_clients: stats.active_clients,
            pending_requests: 0,
            diagnostics_count: 0,
            last_latency_ms: 0,
        })
        .with_gateway(axon_domain::GatewayMetrics {
            active_turn_count: current_active_turns,
        });
    host.stream_bridge().broadcast_observation(&frame);
}

fn handle_host_tick<T>(
    result: Result<T, axon_daemon::DaemonHostError>,
    running: &AtomicBool,
    fenced: &AtomicBool,
    component: &str,
) -> bool {
    match result {
        Ok(_) => true,
        Err(error) if error.is_fenced() => {
            axon_logger::error(&format!(
                "Daemon Host 已失去 lease，停止后台 {component}: {error}"
            ));
            // 多个 Worker 可能同时撞上 fence，置位必须幂等。
            fenced.store(true, Ordering::Release);
            running.store(false, Ordering::Release);
            false
        }
        Err(error) => {
            axon_logger::error(&format!(
                "Daemon {component} tick 失败，将在下一次事件或定时维护重试: {error}"
            ));
            true
        }
    }
}

/// 主 turn 拒绝原因必须留下可诊断证据；用户可见失败由统一 durable terminal 投影。
///
/// 按 error 级留痕：这条日志是「本轮被后端拒绝」唯一能指向真因的地方，而默认
/// `log_level = "error"` 会把 warn 整条吞掉，界面就只剩一句无法自查的结论。
fn handle_primary_turn_failures(
    result: &Result<axon_daemon::PrimaryTurnTickReport, axon_daemon::DaemonHostError>,
) {
    let Ok(report) = result else {
        return;
    };
    for failure in &report.failures {
        axon_logger::error(&format!(
            "Daemon Primary Turn 拒绝 Interaction {}: layer={:?} message={}",
            failure.interaction_id.as_str(),
            failure.layer,
            failure.message
        ));
    }
}

/// 持久 Cron tick：Claim 由 Cron 仓库保证幂等，失败只记录日志并等待下一轮。
fn run_due_cron_jobs(
    adapter: &crate::cron_commands::CronDaemonAdapter,
    scheduler_alive_since_ms: u64,
) {
    if let Err(error) = crate::cron_commands::run_due_cron_jobs(
        adapter,
        SUPERVISOR_BATCH_LIMIT as usize,
        scheduler_alive_since_ms,
    ) {
        axon_logger::warning(&format!(
            "Daemon 持久 Cron tick 失败，将在下一轮重试: {error}"
        ));
    }
}

#[cfg(test)]
mod lease_renew_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn storage_error() -> axon_daemon::DaemonHostError {
        axon_daemon::DaemonHostError::Store(axon_session::RuntimeStoreError::Storage(
            "database is locked".to_owned(),
        ))
    }

    fn running() -> AtomicBool {
        AtomicBool::new(true)
    }

    #[test]
    fn transient_storage_failure_is_retried_until_it_succeeds() {
        let mut calls = 0_u32;
        let outcome = renew_until_fenced(
            || {
                calls = calls.saturating_add(1);
                if calls < 3 {
                    Err(storage_error())
                } else {
                    Ok(())
                }
            },
            &running(),
            Duration::ZERO,
        );

        assert!(
            outcome.is_ok(),
            "写锁抖动必须被重试吃掉，不得停掉后台 Worker"
        );
        assert_eq!(calls, 3);
    }

    #[test]
    fn fenced_lease_stops_immediately_without_retrying() {
        let mut calls = 0_u32;
        let outcome = renew_until_fenced(
            || {
                calls = calls.saturating_add(1);
                Err(axon_daemon::DaemonHostError::Store(
                    axon_session::RuntimeStoreError::Fenced,
                ))
            },
            &running(),
            Duration::ZERO,
        );

        assert!(outcome.is_err());
        assert!(outcome.err().is_some_and(|error| error.is_fenced()));
        assert_eq!(calls, 1, "代际失效不可重试：另一个 Host 已经接管");
    }

    /// 存储长期不可用也不许「放弃」：那正是僵尸 Daemon 的成因。
    ///
    /// 上一版在 8 次重试后直接返回错误并停掉全部 Worker，进程却继续收任务。
    /// 现在只有停机请求（`running` 置假）或代际失效才能让续租线程退出。
    #[test]
    fn persistent_storage_failure_keeps_retrying_until_stop_is_requested() {
        let running = std::sync::Arc::new(AtomicBool::new(true));
        let mut calls = 0_u32;
        let stopper = std::sync::Arc::clone(&running);
        let outcome = renew_until_fenced(
            || {
                calls = calls.saturating_add(1);
                if calls == 5 {
                    stopper.store(false, Ordering::Release);
                }
                Err(storage_error())
            },
            &running,
            Duration::ZERO,
        );

        assert!(outcome.is_ok(), "停机请求是正常退出路径，不是失败");
        assert_eq!(calls, 5, "停机请求必须在下一个重试点生效");
    }

    /// fence 必须交给进程退出判据：只停 Worker 会留下「活着但不干活」的僵尸进程，
    /// 服务管理器的失败重启策略永远不会被触发。
    #[test]
    fn fenced_host_tick_marks_the_host_for_non_zero_exit() {
        let running = running();
        let fenced = AtomicBool::new(false);
        let keep_going = handle_host_tick(
            Err::<(), _>(axon_daemon::DaemonHostError::Store(
                axon_session::RuntimeStoreError::Fenced,
            )),
            &running,
            &fenced,
            "Supervisor",
        );

        assert!(!keep_going, "fence 之后 tick 循环必须退出");
        assert!(
            fenced.load(Ordering::Acquire),
            "fence 结论必须交给退出判据，否则进程会以零退出码留下"
        );
        assert!(
            !running.load(Ordering::Acquire),
            "fence 同时停掉后台 Worker"
        );
    }

    /// 存储抖动只是重试信号：它不是终态，不得把进程推向退出。
    #[test]
    fn transient_tick_failure_does_not_mark_the_host_fenced() {
        let running = running();
        let fenced = AtomicBool::new(false);
        let keep_going = handle_host_tick(
            Err::<(), _>(storage_error()),
            &running,
            &fenced,
            "Supervisor",
        );

        assert!(keep_going, "写锁抖动必须原地重试");
        assert!(!fenced.load(Ordering::Acquire), "抖动不是 fence");
        assert!(running.load(Ordering::Acquire), "抖动不得停掉后台 Worker");
    }
}

#[cfg(test)]
mod checkpoint_tests {
    use super::*;

    /// 体量优先于周期：WAL 越过阈值就必须立刻回写，不能为了等满五分钟让它继续长大；
    /// 体量与周期都没到时一律不动，否则线程会退化成每个 tick 都做一次回写。
    #[test]
    fn oversized_wal_triggers_checkpoint_before_the_interval() {
        let interval = Duration::from_millis(WAL_CHECKPOINT_INTERVAL_MS);

        assert!(
            checkpoint_due(WAL_SIZE_CHECKPOINT_THRESHOLD_BYTES + 1, Duration::ZERO),
            "WAL 已超阈值，即使距上次回写不到五分钟也必须回写"
        );
        assert!(
            !checkpoint_due(WAL_SIZE_CHECKPOINT_THRESHOLD_BYTES - 1, interval / 2),
            "体量与周期都没到，这一轮不该回写"
        );
        assert!(
            !checkpoint_due(WAL_SIZE_CHECKPOINT_THRESHOLD_BYTES, Duration::ZERO),
            "恰好等于阈值不算超过：文件长度可能只是 journal_size_limit 留下的高水位基线"
        );
        assert!(
            checkpoint_due(0, interval),
            "体量虽小也要按周期兜底，否则慢速累积永远等不到阈值"
        );
    }
}
