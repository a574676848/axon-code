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

fn start_lease_worker(host: CliDaemonHost, running: Arc<AtomicBool>) {
    thread::spawn(move || {
        while running.load(Ordering::Acquire) {
            if let Err(error) = host.renew() {
                axon_logger::error(&format!(
                    "Daemon Host 续租失败，后台 Worker 将停止: {error}"
                ));
                running.store(false, Ordering::Release);
                break;
            }
            thread::sleep(Duration::from_millis(HOST_RENEW_INTERVAL_MS));
        }
    });
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
    while running.load(Ordering::Acquire) {
        run_due_cron_jobs(&cron_adapter);
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
fn handle_primary_turn_failures(
    result: &Result<axon_daemon::PrimaryTurnTickReport, axon_daemon::DaemonHostError>,
) {
    let Ok(report) = result else {
        return;
    };
    for failure in &report.failures {
        axon_logger::warning(&format!(
            "Daemon Primary Turn 拒绝 Interaction {}: layer={:?} message={}",
            failure.interaction_id.as_str(),
            failure.layer,
            failure.message
        ));
    }
}

/// 持久 Cron tick：Claim 由 Cron 仓库保证幂等，失败只记录日志并等待下一轮。
fn run_due_cron_jobs(adapter: &crate::remote_commands::FeishuCronAdapter) {
    if let Err(error) =
        crate::cron_commands::run_due_cron_jobs(adapter, SUPERVISOR_BATCH_LIMIT as usize)
    {
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
    match crate::remote_commands::drain_feishu_completion_inbox(
        host,
        axon_root,
        config_path,
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
