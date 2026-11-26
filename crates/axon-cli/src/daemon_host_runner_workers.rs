//! Daemon 后台 Worker 线程的循环与失败处理。

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
    // 写批次由维护循环累加、由回写线程消费：需要的是「自上次回写以来提交了多少批」。
    let write_batches = Arc::new(WriteBatchCounter::default());
    start_primary_turn_worker(
        host.clone(),
        Arc::clone(&workers_running),
        Arc::clone(&host_fenced),
        primary_turn_signal,
        Arc::clone(&maintenance_signal),
        Arc::clone(&write_batches),
    );
    start_supervisor_worker(
        host.clone(),
        active_primary_turns,
        Arc::clone(&workers_running),
        host_fenced,
        maintenance_signal,
        Arc::clone(&write_batches),
    );
    /*
     * WAL 回写单独占一条线程：它要做一次真正的磁盘回写，挂在监督循环里会让每秒一次的
     * 租约续期与维护 tick 一起排到回写后面。独立之后，回写慢只影响自己那一轮。
     */
    start_checkpoint_worker(host.clone(), Arc::clone(&workers_running), write_batches);
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
    write_batches: Arc<WriteBatchCounter>,
) {
    thread::spawn(move || {
        run_primary_turn_loop(
            host,
            running,
            fenced,
            signal,
            maintenance_signal,
            write_batches,
        );
    });
}

fn run_primary_turn_loop(
    host: CliDaemonHost,
    running: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
    write_batches: Arc<WriteBatchCounter>,
) {
    while running.load(Ordering::Acquire) {
        let result = host.primary_turn_tick(SUPERVISOR_BATCH_LIMIT);
        handle_primary_turn_failures(&result);
        if let Ok(report) = &result {
            write_batches.record(&[
                report.claimed,
                report.understood,
                report.rejected,
                report.interrupted,
            ]);
        }
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
    write_batches: Arc<WriteBatchCounter>,
) {
    thread::spawn(move || {
        run_supervisor_loop(
            host,
            active_primary_turns,
            running,
            fenced,
            signal,
            write_batches,
        );
    });
}

fn run_supervisor_loop(
    host: CliDaemonHost,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
    running: Arc<AtomicBool>,
    fenced: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
    write_batches: Arc<WriteBatchCounter>,
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
        let scheduler = host.scheduler_tick(SCHEDULE_LEASE_DURATION_MS, SUPERVISOR_BATCH_LIMIT);
        if let Ok(report) = &scheduler {
            write_batches.record(&[
                report.claimed,
                report.interactions_submitted,
                report.deferred,
                report.failed,
            ]);
        }
        if !handle_host_tick(scheduler, &running, &fenced, "Scheduler") {
            break;
        }
        let supervisor = host.supervisor_tick(SUPERVISOR_BATCH_LIMIT);
        if let Ok(report) = &supervisor {
            write_batches.record(&[
                report.recovered,
                report.quarantined,
                report.interrupted_interactions,
                report.silent_turns_settled,
                report.pumped,
            ]);
        }
        if !handle_host_tick(supervisor, &running, &fenced, "Supervisor") {
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

/// 自上次回写以来，Daemon 维护线程提交过的写批次数。
///
/// 连接池没有统一的提交入口：各仓库取到连接后各自开事务并直接 `commit`，池子看不到提交
/// 动作，只能由写者自己记。维护 tick 的回执里每一条领取、结算、隔离或收敛都对应至少一次
/// 已提交的写事务，因此都按一个写批次累加。这是保守下界：一件工作项正常路径提交两次，
/// 这里只记回执报出来的条目；cron 的回执是格式化摘要，不解析。少记只让回写稍晚（周期
/// 兜底仍在），多记只多一次空回写（`PASSIVE` 拿不到帧就返回），判据方向不受影响。
#[derive(Debug, Default)]
struct WriteBatchCounter {
    batches: AtomicU64,
}

impl WriteBatchCounter {
    /// 记下一个 tick 回执里的写批次数。
    fn record(&self, counts: &[u32]) {
        let total = counts
            .iter()
            .fold(0_u64, |sum, count| sum.saturating_add(u64::from(*count)));
        if total > 0 {
            self.batches.fetch_add(total, Ordering::Relaxed);
        }
    }

    /// 自上次回写以来累计的写批次数。
    fn since_last_checkpoint(&self) -> u64 {
        self.batches.load(Ordering::Relaxed)
    }

    /// 回写之后扣掉已经消费掉的批次。用减法而不是清零：回写期间别的线程可能又提交了
    /// 批次，`swap(0)` 会把它们一起吞掉；计数器只在这里变小，扣减量又来自刚读到的值。
    fn reset_after_checkpoint(&self, handled: u64) {
        if handled > 0 {
            self.batches.fetch_sub(handled, Ordering::Relaxed);
        }
    }
}

/// WAL 回写独立线程：按写批次数或固定周期做一次 `PASSIVE` 回写。
fn start_checkpoint_worker(
    host: CliDaemonHost,
    running: Arc<AtomicBool>,
    write_batches: Arc<WriteBatchCounter>,
) {
    thread::spawn(move || {
        let mut last_checkpoint_at = Instant::now();
        while running.load(Ordering::Acquire) {
            checkpoint_wal_if_due(&host, &write_batches, &mut last_checkpoint_at);
            thread::sleep(Duration::from_millis(CHECKPOINT_WORKER_TICK_MS));
        }
    });
}

/// 回写线程的唤醒粒度：比回写周期短得多，因此只做一次比较就回去睡。
///
/// 它同时是写批次判据的检测延迟上界：批次越过阈值到真正回写，最多晚一个 tick。
const CHECKPOINT_WORKER_TICK_MS: u64 = 30_000;

fn cleanup_releases_if_due(last_release_gc: &mut Instant) {
    if last_release_gc.elapsed() < Duration::from_millis(RELEASE_GC_INTERVAL_MS) {
        return;
    }
    crate::upgrade_commands::cleanup_releases_after_startup();
    *last_release_gc = Instant::now();
}

const LSP_OBSERVATION_SAMPLE_INTERVAL_MS: u64 = 3_000;

/// 写批次触发阈值：自上次回写以来累计到这么多写批次就立即回写，不等固定周期。
///
/// 不再按 `-wal` 文件长度触发：`journal_size_limit` 会把文件截到 64 MiB 的高水位，此后
/// 任意一次写入都让长度严格大于阈值；checkpoint 长期不完整（有长读者）时长度也停在阈值
/// 之上。两种情况下文件长度都不再表达「攒了多少写入」，按长度触发会退化成每个 tick 都
/// 回写一次。批次相反：它是单调累积的提交计数，回写后归零，与高水位无关。
///
/// 为什么取 4096：控制面单个写事务只有几页，4096 批按每批一到数页估算约合数十 MiB，与
/// 旧判据的 64 MiB 同一量级，单次 `PASSIVE` 回写的拷贝量仍落在数百毫秒内（连接池的
/// `busy_timeout` 是 2 秒）。维护 tick 每秒一次、批量上限 32，满载时约一到四分钟就到
/// 4096，计数触发因而先于 5 分钟兜底生效；空闲时批次不累积，判据只由周期驱动。
const WRITE_BATCH_CHECKPOINT_THRESHOLD: u64 = 4096;

/// WAL 回写周期：写批次没到阈值时的兜底。
///
/// 持续写入下 WAL 会一直漂大，漂到与主库同量级后一次全量回写要独占写锁数十秒，所有写者
/// 按 `busy_timeout` 排队失败。周期性 `PASSIVE` 回写把这件事摊平：不阻塞读写，拿不到干净
/// 窗口就留给下一轮。连接池关掉自动回写之后周期性回写只剩这条线程，慢速累积的库等不到
/// 批次阈值，也必须有一次周期性的重置。
const WAL_CHECKPOINT_INTERVAL_MS: u64 = 5 * 60 * 1_000;

/// 这一轮是否该回写：写批次到阈值优先，否则按固定周期兜底。
fn checkpoint_due(write_batches: u64, since_last_checkpoint: Duration) -> bool {
    write_batches >= WRITE_BATCH_CHECKPOINT_THRESHOLD
        || since_last_checkpoint >= Duration::from_millis(WAL_CHECKPOINT_INTERVAL_MS)
}

fn checkpoint_wal_if_due(
    host: &CliDaemonHost,
    write_batches: &WriteBatchCounter,
    last_checkpoint_at: &mut Instant,
) {
    let batches = write_batches.since_last_checkpoint();
    if !checkpoint_due(batches, last_checkpoint_at.elapsed()) {
        return;
    }
    if batches >= WRITE_BATCH_CHECKPOINT_THRESHOLD {
        axon_logger::debug(&format!(
            "自上次回写已提交 {batches} 个写批次，跳过周期等待，立即回写"
        ));
    }
    // 批次与时间一起重置，失败的那一轮也不回填：`PASSIVE` 失败通常是等不到写锁，
    // 下一轮按周期重试即可，留着批次只会让下一轮立刻再撞一次同样的失败。
    *last_checkpoint_at = Instant::now();
    write_batches.reset_after_checkpoint(batches);
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

    /// 周期兜底必须保留：写批次为 0 的空转进程也得周期性重置一次 WAL，否则慢速累积的库
    /// 永远等不到批次阈值，WAL 仍会按写入量一直长。
    #[test]
    fn idle_daemon_still_checkpoints_on_the_interval() {
        let interval = Duration::from_millis(WAL_CHECKPOINT_INTERVAL_MS);

        assert!(
            checkpoint_due(0, interval),
            "一个写批次都没有也要按周期兜底，否则慢速累积永远等不到阈值"
        );
        assert!(
            !checkpoint_due(0, interval / 2),
            "批次与周期都没到，这一轮不该回写"
        );
    }

    /// 批次到阈值就是「攒够了」：不能为了等满五分钟继续让它攒下去。
    #[test]
    fn enough_write_batches_trigger_before_the_interval() {
        assert!(
            checkpoint_due(WRITE_BATCH_CHECKPOINT_THRESHOLD, Duration::ZERO),
            "批次刚到阈值，即使才回写过不久也必须回写"
        );
        assert!(
            checkpoint_due(WRITE_BATCH_CHECKPOINT_THRESHOLD + 1, Duration::ZERO),
            "越过阈值之后每一轮都该回写，直到批次被扣掉"
        );
    }

    /// 批次不足且未到间隔时一律不动：否则线程会退化成每个 tick 都做一次空回写。
    #[test]
    fn insufficient_write_batches_before_the_interval_do_not_trigger() {
        let interval = Duration::from_millis(WAL_CHECKPOINT_INTERVAL_MS);

        assert!(
            !checkpoint_due(WRITE_BATCH_CHECKPOINT_THRESHOLD - 1, interval / 2),
            "差一个批次也还没到，这一轮不该回写"
        );
        assert!(
            !checkpoint_due(1, Duration::ZERO),
            "刚回写过就只提交了一批，不该立刻再回写"
        );
    }

    /// 回写之后批次归零；回写期间新到的批次不能被一起吞掉。
    #[test]
    fn checkpoint_resets_the_write_batch_counter() {
        let counter = WriteBatchCounter::default();
        counter.record(&[WRITE_BATCH_CHECKPOINT_THRESHOLD as u32]);
        let observed = counter.since_last_checkpoint();
        assert_eq!(observed, WRITE_BATCH_CHECKPOINT_THRESHOLD);
        assert!(checkpoint_due(observed, Duration::ZERO));

        // 回写进行中另一条线程又提交了两批：扣减只能扣掉本轮读到的那部分。
        counter.record(&[2]);
        counter.reset_after_checkpoint(observed);

        assert_eq!(
            counter.since_last_checkpoint(),
            2,
            "回写期间新到的批次必须留到下一轮，不能随重置一起清掉"
        );
        assert!(
            !checkpoint_due(counter.since_last_checkpoint(), Duration::ZERO),
            "归零之后必须再攒够一批才触发"
        );
    }
}
