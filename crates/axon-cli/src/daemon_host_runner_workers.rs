//! Daemon 后台 Worker 线程的循环与失败处理。

use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    CRON_TICK_INTERVAL_MS, CliDaemonHost, DaemonMaintenanceSignal, DaemonRunCommand,
    FEISHU_DELIVERY_RECOVERY_INTERVAL_MS, HOST_RENEW_INTERVAL_MS, MAINTENANCE_TIMER_INTERVAL_MS,
    RELEASE_GC_INTERVAL_MS, SCHEDULE_LEASE_DURATION_MS, SUPERVISOR_BATCH_LIMIT,
};

pub(super) fn start_background_workers(
    command: &DaemonRunCommand,
    host: &CliDaemonHost,
    cron_adapter: crate::remote_commands::FeishuCronAdapter,
    delivery_owner: String,
    workers_running: Arc<AtomicBool>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
    primary_turn_signal: Arc<DaemonMaintenanceSignal>,
    delivery_signal: Arc<DaemonMaintenanceSignal>,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
) {
    start_lease_worker(host.clone(), Arc::clone(&workers_running));
    start_primary_turn_worker(
        host.clone(),
        Arc::clone(&workers_running),
        primary_turn_signal,
        Arc::clone(&maintenance_signal),
    );
    start_supervisor_worker(
        host.clone(),
        active_primary_turns,
        Arc::clone(&workers_running),
        maintenance_signal,
    );
    start_cron_worker(cron_adapter, Arc::clone(&workers_running));
    start_delivery_worker(
        host.clone(),
        command.root.clone(),
        command.config_path.clone(),
        delivery_owner,
        Arc::clone(&workers_running),
        delivery_signal,
    );
}

/// 两次续租重试之间的退避。
const LEASE_RENEW_RETRY_DELAY: Duration = Duration::from_millis(750);
/// 持续失败的告警节流：每约 30 秒留一条 WARNING，风暴期间不刷日志。
const LEASE_RENEW_WARN_EVERY: u32 = 40;

fn start_lease_worker(host: CliDaemonHost, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::Acquire) {
            if let Err(error) =
                renew_until_fenced(|| host.renew(), &running, LEASE_RENEW_RETRY_DELAY)
            {
                axon_logger::error(&format!(
                    "Daemon Host 已失去 lease，后台 Worker 将停止: {error}"
                ));
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
    signal: Arc<DaemonMaintenanceSignal>,
    maintenance_signal: Arc<DaemonMaintenanceSignal>,
) {
    thread::spawn(move || {
        run_primary_turn_loop(host, running, signal, maintenance_signal);
    });
}

fn run_primary_turn_loop(
    host: CliDaemonHost,
    running: Arc<AtomicBool>,
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
        if !handle_host_tick(result, &running, "Primary Turn") {
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
    signal: Arc<DaemonMaintenanceSignal>,
) {
    thread::spawn(move || run_supervisor_loop(host, active_primary_turns, running, signal));
}

fn run_supervisor_loop(
    host: CliDaemonHost,
    active_primary_turns: crate::run_commands::run_steer::ActivePrimaryTurnRegistry,
    running: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
) {
    let mut last_release_gc = Instant::now();
    let mut last_lsp_sample = Instant::now();
    let mut last_wal_checkpoint = Instant::now();
    let mut last_lsp_active = None;
    let mut last_active_turns = None;
    while running.load(Ordering::Acquire) {
        cleanup_releases_if_due(&mut last_release_gc);
        checkpoint_wal_if_due(&host, &mut last_wal_checkpoint);
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
            "Scheduler",
        ) || !handle_host_tick(
            host.supervisor_tick(SUPERVISOR_BATCH_LIMIT),
            &running,
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
    cron_adapter: crate::remote_commands::FeishuCronAdapter,
    running: Arc<AtomicBool>,
) {
    thread::spawn(move || run_cron_loop(cron_adapter, running));
}

fn run_cron_loop(
    cron_adapter: crate::remote_commands::FeishuCronAdapter,
    running: Arc<AtomicBool>,
) {
    // 停机期间错过的触发点不补跑：只有本进程在线期间到期的任务才会执行。
    let scheduler_alive_since_ms = axon_time::unix_now_ms();
    while running.load(Ordering::Acquire) {
        run_due_cron_jobs(&cron_adapter, scheduler_alive_since_ms);
        thread::sleep(Duration::from_millis(CRON_TICK_INTERVAL_MS));
    }
}

fn cleanup_releases_if_due(last_release_gc: &mut Instant) {
    if last_release_gc.elapsed() < Duration::from_millis(RELEASE_GC_INTERVAL_MS) {
        return;
    }
    crate::upgrade_commands::cleanup_releases_after_startup();
    *last_release_gc = Instant::now();
}

const LSP_OBSERVATION_SAMPLE_INTERVAL_MS: u64 = 3_000;

/// WAL 回写周期。
///
/// 持续写入下 WAL 会一直漂大，漂到与主库同量级后一次全量回写要独占写锁数十秒，
/// 所有写者按 `busy_timeout` 排队失败，续租与业务写一起被拉下水。周期性地做
/// `PASSIVE` 回写把这件事摊平：不阻塞读写，拿不到干净窗口就留给下一轮。
const WAL_CHECKPOINT_INTERVAL_MS: u64 = 5 * 60 * 1_000;

fn checkpoint_wal_if_due(host: &CliDaemonHost, last_checkpoint_at: &mut Instant) {
    if last_checkpoint_at.elapsed() < Duration::from_millis(WAL_CHECKPOINT_INTERVAL_MS) {
        return;
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
    component: &str,
) -> bool {
    match result {
        Ok(_) => true,
        Err(error) if error.is_fenced() => {
            axon_logger::error(&format!(
                "Daemon Host 已失去 lease，停止后台 {component}: {error}"
            ));
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
    adapter: &crate::remote_commands::FeishuCronAdapter,
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

fn start_delivery_worker(
    host: CliDaemonHost,
    axon_root: PathBuf,
    config_path: PathBuf,
    owner: String,
    running: Arc<AtomicBool>,
    signal: Arc<DaemonMaintenanceSignal>,
) {
    thread::spawn(move || {
        while running.load(Ordering::Acquire) {
            run_delivery_tick(&config_path, &owner);
            run_feishu_inbox_tick(&host, &axon_root, &config_path);
            signal.wait(Duration::from_millis(FEISHU_DELIVERY_RECOVERY_INTERVAL_MS));
        }
    });
}

/// 排空持久收件箱里等待飞书投递的完成消息。
///
/// 与出站恢复共用同一 tick：完成消息先进入收件箱，再由本 tick 写入 durable outbox，
/// 顺序写入 + 幂等 dispatch 保证同一条完成消息不会重复送达，也不会被静默确认。
fn run_feishu_inbox_tick(
    host: &CliDaemonHost,
    axon_root: &std::path::Path,
    config_path: &std::path::Path,
) {
    let allow_outbound = crate::remote_commands::feishu_channel_enabled(config_path);
    match crate::remote_commands::drain_feishu_completion_inbox(
        host,
        axon_root,
        allow_outbound,
        SUPERVISOR_BATCH_LIMIT,
    ) {
        Ok(0) => {}
        Ok(delivered) => axon_logger::info(&format!(
            "飞书完成消息投递器已确认送达 {delivered} 条收件箱条目"
        )),
        Err(error) => axon_logger::warning(&format!(
            "飞书完成消息投递器 tick 失败，将在下一轮重试: {error}"
        )),
    }
}

fn run_delivery_tick(config_path: &std::path::Path, owner: &str) {
    if let Err(error) = crate::remote_commands::run_daemon_delivery_tick(
        config_path,
        owner,
        SUPERVISOR_BATCH_LIMIT as usize,
    ) {
        axon_logger::warning(&format!(
            "Feishu Daemon delivery tick 失败，将在下一轮重试: {error}"
        ));
    }
    if let Err(error) = crate::remote_commands::run_daemon_feedback_tick(
        config_path,
        SUPERVISOR_BATCH_LIMIT as usize,
    ) {
        axon_logger::warning(&format!(
            "Feishu Daemon feedback tick 失败，将在下一轮重试: {error}"
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
}
