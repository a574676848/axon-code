use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use super::{
    RemoteCliContext,
    remote_feishu_gateway::remote_feishu_gateway_execution::start_gateway_lane_scheduler,
    remote_feishu_gateway_closure::{
        FeishuGatewayClosureReport, accumulate_closure_report, build_gateway_daemon_command,
        run_feishu_gateway_closure_tick,
    },
    remote_feishu_question_card_tick::FeishuQuestionCardTickRuntime,
};

pub(super) const GATEWAY_DRAIN_FILE_NAME: &str = ".gateway.drain";
const GATEWAY_LEGACY_STOP_FILE_NAME: &str = ".gateway.stop";
const GATEWAY_STOP_POLL_MS: u64 = 250;
const GATEWAY_RECOVERY_SCAN_INTERVAL_MS: u64 = 5_000;
/// 问答卡片 tick 的最小网络间隔：提问是低频事实，不需要跟随 250ms 轮询。
const GATEWAY_QUESTION_CARD_TICK_INTERVAL_MS: u64 = 3_000;

/// 闭环 worker 的轮询节奏。
///
/// 生产用默认值；测试用短节奏，避免为了等一次「无唤醒也会自己动」的回收扫描
/// 而让用例耗时数秒。
#[derive(Debug, Clone, Copy)]
pub(in crate::remote_commands) struct GatewayClosureTiming {
    /// 无唤醒时的等待切片；也决定回收扫描的时间粒度。
    pub(in crate::remote_commands) stop_poll: Duration,
    /// 无唤醒时最长多久必须主动做一次回收扫描。
    pub(in crate::remote_commands) recovery_scan_interval: Duration,
}

impl GatewayClosureTiming {
    /// 生产节奏：250ms 切片、5s 主动扫描。
    pub(in crate::remote_commands) const PRODUCTION: Self = Self {
        stop_poll: Duration::from_millis(GATEWAY_STOP_POLL_MS),
        recovery_scan_interval: Duration::from_millis(GATEWAY_RECOVERY_SCAN_INTERVAL_MS),
    };
}

impl Default for GatewayClosureTiming {
    fn default() -> Self {
        Self::PRODUCTION
    }
}

pub(super) struct FeishuGatewayBackgroundWorker {
    wake: SyncSender<()>,
    cancelled: Arc<AtomicBool>,
    totals: Arc<Mutex<FeishuGatewayClosureReport>>,
    thread: Option<JoinHandle<()>>,
}

impl FeishuGatewayBackgroundWorker {
    pub(super) fn start_with_cancellation(
        context: RemoteCliContext,
        session_id: u64,
        event_source: &Path,
        tenant_access_token: String,
        api_base_url: Option<String>,
        _execute_feedback_inline: bool,
        cancellation: Option<Arc<AtomicBool>>,
        timing: GatewayClosureTiming,
    ) -> Self {
        let (wake, receiver) = mpsc::sync_channel(1);
        let cancelled = cancellation.unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let worker_cancelled = Arc::clone(&cancelled);
        let totals = Arc::new(Mutex::new(FeishuGatewayClosureReport::default()));
        let worker_totals = Arc::clone(&totals);
        let runtime_root = context.root.clone();
        let command = build_gateway_daemon_command(session_id, event_source);
        // 问答卡片 tick 与车道共享同一份远端上下文：客户端只在首个 tick 建立一次。
        let mut question_cards = FeishuQuestionCardTickRuntime::new(
            context.clone(),
            GATEWAY_QUESTION_CARD_TICK_INTERVAL_MS,
        );
        // 车道调度器随 worker 线程一起存活：车道完成会唤醒本线程继续结算与读取。
        let scheduler = start_gateway_lane_scheduler(
            context,
            command.clone(),
            tenant_access_token,
            api_base_url,
            wake.clone(),
        );
        let thread = thread::spawn(move || {
            let recovery_scan_target_ms = millis(timing.recovery_scan_interval);
            let mut recovery_elapsed_ms = recovery_scan_target_ms;
            while !worker_cancelled.load(Ordering::SeqCst) {
                let should_drain = match receiver.recv_timeout(timing.stop_poll) {
                    Ok(()) => true,
                    Err(RecvTimeoutError::Timeout) => {
                        recovery_elapsed_ms =
                            recovery_elapsed_ms.saturating_add(millis(timing.stop_poll));
                        recovery_elapsed_ms >= recovery_scan_target_ms
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                };
                if gateway_drain_requested(&runtime_root) {
                    let _ = super::remote_feishu_gateway_status::GatewayRuntimeStatus::transition(
                        &runtime_root,
                        std::process::id(),
                        super::remote_feishu_gateway_status::GatewayRuntimeState::Drained,
                        None,
                        None,
                    );
                    worker_cancelled.store(true, Ordering::SeqCst);
                    break;
                }
                // 事件源是分片 JSONL：逻辑路径（`…_events.jsonl`）**永远不作为文件存在**，
                // 物理分片是 `…_events.000000.jsonl`。因此这里绝不能用 `exists()` 当门槛：
                // 它会恒为假，使读取被永久跳过（真机事故：消息已 durable 入队并挂上 Typing
                // 反应，却再没有任何后续，`activeTaskCount` 恒为 0、cursor 永不生成）。
                // 「还没有数据」由读取层自然表达：空目录返回空列表、空批次不推进偏移。
                if !should_drain {
                    continue;
                }
                recovery_elapsed_ms = 0;
                // 问答卡片 tick：投递 Daemon 侧新产生的提问，并收口已在别处结算的卡片。
                if let Some(Ok(report)) = question_cards.tick()
                    && (report.delivered_cards > 0 || report.settled_cards > 0)
                {
                    axon_logger::debug(&format!(
                        "Feishu 问答卡片 tick: delivered={}, settled={}, inspected_lanes={}",
                        report.delivered_cards, report.settled_cards, report.inspected_lanes
                    ));
                }
                let pid = std::process::id();
                let _ = super::remote_feishu_gateway_status::GatewayRuntimeStatus::update_active_task_count(
                    &runtime_root,
                    pid,
                    1,
                );
                match run_feishu_gateway_closure_tick(&command, &scheduler) {
                    Ok(report) => {
                        if let Ok(mut aggregate) = worker_totals.lock() {
                            accumulate_closure_report(&mut aggregate, &report);
                        }
                    }
                    Err(error) => axon_logger::warning(&format!(
                        "Feishu gateway 后台闭环失败，将自动重试: {error}"
                    )),
                }
                let _ = super::remote_feishu_gateway_status::GatewayRuntimeStatus::update_active_task_count(
                    &runtime_root,
                    pid,
                    0,
                );
            }
        });
        Self {
            wake,
            cancelled,
            totals,
            thread: Some(thread),
        }
    }

    pub(super) fn wake_sender(&self) -> SyncSender<()> {
        self.wake.clone()
    }

    pub(super) fn cancellation(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    pub(super) fn snapshot(&self) -> FeishuGatewayClosureReport {
        self.totals
            .lock()
            .map(|totals| totals.clone())
            .unwrap_or_default()
    }
}

impl Drop for FeishuGatewayBackgroundWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            // 线程退出时其闭包释放车道调度器，先 join 才能保证车道停止后再结束 worker。
            let _ = thread.join();
        }
    }
}

pub(super) fn gateway_drain_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join(GATEWAY_DRAIN_FILE_NAME)
}

/// 把时长折算成毫秒；用于回收扫描的累加判定。
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

pub(super) fn gateway_drain_requested(runtime_root: &Path) -> bool {
    gateway_drain_path(runtime_root).exists()
        || runtime_root.join(GATEWAY_LEGACY_STOP_FILE_NAME).exists()
}
