//! Feishu gateway 入站事件的有序车道调度。
//!
//! 读源与处理解耦：调度线程只按事件源顺序读取并投递，固定数量的 worker 执行
//! 「enrich → 治理 → Interaction 提交」。同一 chat 严格串行、不同 chat 并行；
//! durable cursor 只在整批完成后按批次顺序推进。未确认区间在重启后按事件源顺序重放，
//! 治理记录与 Interaction ID 保证重放幂等，因此不需要新的持久化状态。

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::super::RemoteFeishuGatewayCliEventReport;
use super::super::remote_feishu_gateway_input::FeishuGatewayInputEvent;
use super::remote_feishu_gateway_event_execution::ExecutedGatewayInput;

/// 车道 worker 数量。
///
/// 与远端状态 SQLite 的每路径空闲连接上限（4）和 primary turn 并发 lane 数对齐：
/// 更多 worker 只会让连接反复创建/关闭，并把写锁争用变成额外开销。
pub(super) const GATEWAY_LANE_WORKER_COUNT: usize = 4;
/// 未确认行窗口上界；达到上限即停止读取，用 durable 事件源充当背压载体。
pub(super) const GATEWAY_LANE_MAX_UNACKED_LINES: u64 = 64;
/// 单次失败后的重试基础退避。
const GATEWAY_LANE_RETRY_BACKOFF: Duration = Duration::from_secs(1);
/// 重试退避上限，避免持续失败的车道长期占用 worker。
const GATEWAY_LANE_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// 空闲等待周期：同时充当退避到期与取消的检查粒度。
const GATEWAY_LANE_IDLE_POLL: Duration = Duration::from_millis(250);
/// 退避等待的取消检查粒度，决定 drain/退出时最坏等待时间。
const GATEWAY_LANE_CANCEL_SLICE: Duration = Duration::from_millis(100);
/// 单个 tick 最多带出的完成报告数，保持汇报通道有界。
const GATEWAY_LANE_REPORT_CAPACITY: usize = 256;
/// 车道任务等待时长警戒值（从入队到开始执行）。
const GATEWAY_LANE_WAIT_WARN_MS: u64 = 5_000;
/// 单条车道任务执行时长警戒值。
const GATEWAY_LANE_TASK_WARN_MS: u64 = 10_000;
/// 无法在投递时确定 chat 归属的事件（例如还未补齐 chat_id 的 reaction）使用的专用车道。
///
/// 这类事件在同一车道内按事件源顺序串行，不会与某个 chat 的事件并发推进同一条
/// ingress 水位；补齐归属后仍由治理链按真实 chat 记账。
const GATEWAY_LANE_UNRESOLVED: &str = "unresolved";

type LaneRunner =
    Arc<dyn Fn(&FeishuGatewayInputEvent) -> Result<ExecutedGatewayInput, String> + Send + Sync>;

/// 一条待执行的车道任务；携带所属批次以便按批归属结算。
struct LaneTask {
    batch_end_line: u64,
    /// 入队时刻；用于观测车道等待时长。
    enqueued_at: Instant,
    input: FeishuGatewayInputEvent,
}

#[derive(Default)]
struct SchedulerState {
    /// 每个 chat 一条 FIFO；队列空即移除，车道不会泄漏。
    lanes: BTreeMap<String, VecDeque<LaneTask>>,
    /// 正在执行中的车道；同一车道同时只有一个事件在跑。
    busy: HashSet<String>,
    /// 批次结束行号 → 未完成事件数；按行号有序，便于找连续确认前缀。
    pending: BTreeMap<u64, usize>,
    read_offset: u64,
    acked_offset: u64,
    reports: Vec<RemoteFeishuGatewayCliEventReport>,
    completed: usize,
    persisted_records: usize,
    released_turns: usize,
    max_queue_depth: usize,
    max_wait_ms: u64,
    max_task_ms: u64,
    saturated_ticks: usize,
    saturated: bool,
}

impl SchedulerState {
    /// 取出下一条可执行任务：跳过正在执行的车道，保证 chat 内全序。
    fn take_ready(&mut self) -> Option<(String, LaneTask)> {
        let lane = self
            .lanes
            .iter()
            .find(|(lane, queue)| !queue.is_empty() && !self.busy.contains(*lane))
            .map(|(lane, _)| lane.clone())?;
        let task = self.lanes.get_mut(&lane)?.pop_front()?;
        if self.lanes.get(&lane).is_some_and(VecDeque::is_empty) {
            self.lanes.remove(&lane);
        }
        self.busy.insert(lane.clone());
        Some((lane, task))
    }

    /// 按行号顺序推进「整批完成」的连续确认前缀。
    ///
    /// 只有队首批次可以推进：空洞会让未处理事件被 cursor 跳过。
    fn advance_acked_prefix(&mut self) {
        while let Some(entry) = self.pending.first_entry() {
            if *entry.get() != 0 {
                break;
            }
            let end_line = *entry.key();
            self.pending.remove(&end_line);
            self.acked_offset = end_line;
        }
    }

    /// 结算一条成功任务：按批次归属扣减，再推进连续确认前缀。
    fn finish_task(
        &mut self,
        lane: &str,
        batch_end_line: u64,
        waited_ms: u64,
        task_ms: u64,
        executed: ExecutedGatewayInput,
    ) {
        self.busy.remove(lane);
        self.completed += 1;
        self.max_wait_ms = self.max_wait_ms.max(waited_ms);
        self.max_task_ms = self.max_task_ms.max(task_ms);
        self.persisted_records += executed.persisted_records;
        self.released_turns += executed.released_turns;
        self.reports.extend(executed.event_reports);
        let overflow = self
            .reports
            .len()
            .saturating_sub(GATEWAY_LANE_REPORT_CAPACITY);
        if overflow > 0 {
            self.reports.drain(..overflow);
        }
        if let Some(remaining) = self.pending.get_mut(&batch_end_line) {
            *remaining = remaining.saturating_sub(1);
        }
        self.advance_acked_prefix();
    }
}

struct SchedulerInner {
    run: LaneRunner,
    /// 车道完成即唤醒调度线程继续结算与读取；复用既有有界合并唤醒通道。
    wake_dispatcher: SyncSender<()>,
    cancelled: AtomicBool,
    state: Mutex<SchedulerState>,
    ready: Condvar,
}

impl SchedulerInner {
    fn lock(&self) -> MutexGuard<'_, SchedulerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn wake(&self) {
        let _ = self.wake_dispatcher.try_send(());
    }
}

/// 一次 tick 可带出的调度结算结果。
#[derive(Default)]
pub(crate) struct GatewayLaneCompletion {
    pub(crate) acked_offset: u64,
    pub(crate) completed: usize,
    pub(crate) persisted_records: usize,
    pub(crate) released_turns: usize,
    /// 本窗口结束时的未确认行数：它决定读取是否已被背压暂停。
    pub(crate) acked_lag_lines: u64,
    /// 本窗口内车道队列的峰值深度。
    pub(crate) max_queue_depth: usize,
    /// 本窗口内单条车道任务的最大等待时长（入队到开始执行）。
    pub(crate) max_wait_ms: u64,
    /// 本窗口内单条车道任务的最大执行时长。
    pub(crate) max_task_ms: u64,
    /// 累计「读窗口已满」的 tick 数。
    pub(crate) saturated_ticks: usize,
    pub(crate) event_reports: Vec<RemoteFeishuGatewayCliEventReport>,
}

/// 按 chat 分车道的有界调度器。
pub(crate) struct GatewayLaneScheduler {
    inner: Arc<SchedulerInner>,
    workers: Vec<JoinHandle<()>>,
}

impl GatewayLaneScheduler {
    /// 启动调度器；`run` 执行单条事件的完整处理链。
    pub(in crate::remote_commands::remote_feishu_gateway) fn start<F>(
        run: F,
        wake_dispatcher: SyncSender<()>,
    ) -> Self
    where
        F: Fn(&FeishuGatewayInputEvent) -> Result<ExecutedGatewayInput, String>
            + Send
            + Sync
            + 'static,
    {
        let inner = Arc::new(SchedulerInner {
            run: Arc::new(run),
            wake_dispatcher,
            cancelled: AtomicBool::new(false),
            state: Mutex::new(SchedulerState::default()),
            ready: Condvar::new(),
        });
        let workers = (0..GATEWAY_LANE_WORKER_COUNT)
            .filter_map(|index| {
                let inner = Arc::clone(&inner);
                thread::Builder::new()
                    .name(format!("feishu-gateway-lane-{index}"))
                    .spawn(move || run_lane_worker(inner))
                    .ok()
            })
            .collect();
        Self { inner, workers }
    }

    /// 下一次读取应使用的行偏移。
    pub(crate) fn read_offset(&self) -> u64 {
        self.inner.lock().read_offset
    }

    /// 未确认行数达到上界时停止读取。
    pub(crate) fn can_accept_batch(&self) -> bool {
        let state = self.inner.lock();
        state.read_offset.saturating_sub(state.acked_offset) < GATEWAY_LANE_MAX_UNACKED_LINES
    }

    /// 调度器内待处理（排队 + 执行中）的事件数。
    pub(crate) fn in_flight_events(&self) -> usize {
        let state = self.inner.lock();
        state.lanes.values().map(VecDeque::len).sum::<usize>() + state.busy.len()
    }

    /// 投递一个已读取批次。
    ///
    /// 允许空事件批次：整批都是坏行时仍要推进读偏移与确认前缀，否则读取会永久卡住。
    /// 事件按 chat 归属入队；归属未知的事件进入专用车道。
    pub(in crate::remote_commands::remote_feishu_gateway) fn dispatch(
        &self,
        batch_end_line: u64,
        events: Vec<FeishuGatewayInputEvent>,
    ) {
        {
            let mut state = self.inner.lock();
            let now = Instant::now();
            state.pending.insert(batch_end_line, events.len());
            for event in events {
                state
                    .lanes
                    .entry(lane_key(&event))
                    .or_default()
                    .push_back(LaneTask {
                        batch_end_line,
                        enqueued_at: now,
                        input: event,
                    });
            }
            state.read_offset = batch_end_line;
            let depth = state.lanes.values().map(VecDeque::len).sum::<usize>() + state.busy.len();
            state.max_queue_depth = state.max_queue_depth.max(depth);
            // 整批都是坏行时没有任何任务会完成，必须在这里立即确认，否则读取永久卡住。
            state.advance_acked_prefix();
        }
        self.inner.ready.notify_all();
    }

    /// 正在执行事件的车道数量。
    pub(crate) fn active_lanes(&self) -> usize {
        self.inner.lock().busy.len()
    }

    /// 记录本 tick 的读窗口状态；返回是否「刚进入饱和」，用于只告警一次。
    pub(crate) fn note_read_window(&self, saturated: bool) -> bool {
        let mut state = self.inner.lock();
        if saturated {
            state.saturated_ticks += 1;
        }
        let entered = saturated && !state.saturated;
        state.saturated = saturated;
        entered
    }

    /// 取走本 tick 累积的结算结果。
    pub(crate) fn take_completion(&self) -> GatewayLaneCompletion {
        let mut state = self.inner.lock();
        GatewayLaneCompletion {
            acked_offset: state.acked_offset,
            completed: std::mem::take(&mut state.completed),
            persisted_records: std::mem::take(&mut state.persisted_records),
            released_turns: std::mem::take(&mut state.released_turns),
            acked_lag_lines: state.read_offset.saturating_sub(state.acked_offset),
            max_queue_depth: std::mem::take(&mut state.max_queue_depth),
            max_wait_ms: std::mem::take(&mut state.max_wait_ms),
            max_task_ms: std::mem::take(&mut state.max_task_ms),
            saturated_ticks: state.saturated_ticks,
            event_reports: std::mem::take(&mut state.reports),
        }
    }

    /// 事件源被整体回收后行号归零。
    pub(crate) fn reset_after_source_compaction(&self) {
        let mut state = self.inner.lock();
        if !state.pending.is_empty() {
            return;
        }
        state.read_offset = 0;
        state.acked_offset = 0;
    }
}

impl Drop for GatewayLaneScheduler {
    fn drop(&mut self) {
        self.inner.cancelled.store(true, Ordering::Release);
        self.inner.ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

/// 事件的车道归属；chat 未知时使用专用车道，避免伪装成某条 chat 车道。
fn lane_key(event: &FeishuGatewayInputEvent) -> String {
    if event.chat_id.trim().is_empty() {
        return GATEWAY_LANE_UNRESOLVED.to_owned();
    }
    event.chat_id.clone()
}

fn run_lane_worker(inner: Arc<SchedulerInner>) {
    loop {
        let Some((lane, task)) = wait_for_task(&inner) else {
            return;
        };
        let input = task.input;
        let batch_end_line = task.batch_end_line;
        let started = Instant::now();
        let waited_ms = millis(started.saturating_duration_since(task.enqueued_at));
        let mut attempt = 0u32;
        loop {
            match (inner.run)(&input) {
                Ok(executed) => {
                    let task_ms = millis(started.elapsed());
                    if waited_ms >= GATEWAY_LANE_WAIT_WARN_MS
                        || task_ms >= GATEWAY_LANE_TASK_WARN_MS
                    {
                        axon_logger::warning(&format!(
                            "Feishu gateway 车道任务耗时超过警戒值: lane={lane}, waited_ms={waited_ms}, task_ms={task_ms}, attempt={attempt}"
                        ));
                    }
                    inner
                        .lock()
                        .finish_task(&lane, batch_end_line, waited_ms, task_ms, executed);
                    inner.ready.notify_all();
                    inner.wake();
                    break;
                }
                Err(error) => {
                    attempt = attempt.saturating_add(1);
                    let backoff = retry_backoff(attempt);
                    axon_logger::warning(&format!(
                        "Feishu gateway 车道任务失败，将退避后重试: lane={lane}, attempt={attempt}, backoff_ms={}, error={error}",
                        backoff.as_millis()
                    ));
                    if sleep_cancellable(&inner, backoff) {
                        // 取消即放弃该任务：批次未确认，重启后按事件源顺序重放。
                        return;
                    }
                }
            }
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn wait_for_task(inner: &Arc<SchedulerInner>) -> Option<(String, LaneTask)> {
    let mut state = inner.lock();
    loop {
        if inner.cancelled.load(Ordering::Acquire) {
            return None;
        }
        if let Some(task) = state.take_ready() {
            return Some(task);
        }
        let (next, _) = inner
            .ready
            .wait_timeout(state, GATEWAY_LANE_IDLE_POLL)
            .unwrap_or_else(PoisonError::into_inner);
        state = next;
    }
}

/// 可取消的退避等待；返回 `true` 表示已请求取消。
fn sleep_cancellable(inner: &Arc<SchedulerInner>, delay: Duration) -> bool {
    let mut remaining = delay;
    while !remaining.is_zero() {
        if inner.cancelled.load(Ordering::Acquire) {
            return true;
        }
        let slice = remaining.min(GATEWAY_LANE_CANCEL_SLICE);
        thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
    inner.cancelled.load(Ordering::Acquire)
}

fn retry_backoff(attempt: u32) -> Duration {
    let factor = 1u32 << attempt.min(5);
    GATEWAY_LANE_RETRY_BACKOFF
        .saturating_mul(factor)
        .min(GATEWAY_LANE_RETRY_BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicUsize, mpsc};

    use super::*;

    fn event(chat_id: &str, request_id: &str) -> FeishuGatewayInputEvent {
        serde_json::from_value(serde_json::json!({
            "kind": "message",
            "requestId": request_id,
            "chatId": chat_id,
            "chatType": "p2p",
            "messageId": format!("om_{request_id}"),
            "sourceCreatedAtMs": 100,
            "senderOpenId": "ou_sender",
            "text": request_id,
        }))
        .expect("测试事件应有效")
    }

    fn executed() -> ExecutedGatewayInput {
        ExecutedGatewayInput {
            persisted_records: 1,
            released_turns: 0,
            event_reports: Vec::new(),
        }
    }

    fn scheduler<F>(run: F) -> (GatewayLaneScheduler, mpsc::Receiver<()>)
    where
        F: Fn(&FeishuGatewayInputEvent) -> Result<ExecutedGatewayInput, String>
            + Send
            + Sync
            + 'static,
    {
        let (wake, receiver) = mpsc::sync_channel(16);
        (GatewayLaneScheduler::start(run, wake), receiver)
    }

    #[test]
    fn same_chat_events_run_in_dispatch_order() {
        let (order_sender, order_receiver) = mpsc::channel();
        let (scheduler, _wake) = scheduler(move |input| {
            order_sender.send(input.request_id.clone()).ok();
            Ok(executed())
        });

        scheduler.dispatch(3, vec![event("oc_a", "req-1"), event("oc_a", "req-2")]);
        scheduler.dispatch(4, vec![event("oc_a", "req-3")]);

        let order = (0..3)
            .map(|_| {
                order_receiver
                    .recv_timeout(Duration::from_secs(2))
                    .expect("同 chat 事件应按序完成")
            })
            .collect::<Vec<_>>();
        assert_eq!(order, vec!["req-1", "req-2", "req-3"]);
    }

    #[test]
    fn different_chats_run_concurrently() {
        let concurrent = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let (started_sender, started_receiver) = mpsc::channel::<()>();
        let started = Arc::new(Mutex::new(started_receiver));
        let worker_concurrent = Arc::clone(&concurrent);
        let worker_peak = Arc::clone(&peak);
        let worker_started = Arc::clone(&started);
        let (scheduler, _wake) = scheduler(move |input| {
            let now = worker_concurrent.fetch_add(1, Ordering::SeqCst) + 1;
            worker_peak.fetch_max(now, Ordering::SeqCst);
            if input.request_id == "req-a" {
                // 等待另一条车道进入执行；超时即说明跨 chat 没有并行。
                let receiver = worker_started
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let _ = receiver.recv_timeout(Duration::from_secs(2));
            } else {
                let _ = started_sender.send(());
            }
            worker_concurrent.fetch_sub(1, Ordering::SeqCst);
            Ok(executed())
        });

        scheduler.dispatch(2, vec![event("oc_a", "req-a"), event("oc_b", "req-b")]);

        assert!(
            wait_for_acked(&scheduler, 2, Duration::from_secs(5)),
            "跨 chat 事件必须完成"
        );
        assert!(peak.load(Ordering::SeqCst) >= 2, "跨 chat 事件必须并行执行");
    }

    #[test]
    fn cursor_advances_only_over_the_contiguous_completed_prefix() {
        let (release, released) = mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(Some(released)));
        let slow_holder = Arc::clone(&held);
        let (scheduler, _wake) = scheduler(move |input| {
            if input.request_id == "req-slow" {
                let receiver = slow_holder
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .expect("只持有一条慢车道");
                let _ = receiver.recv_timeout(Duration::from_secs(5));
            }
            Ok(executed())
        });

        scheduler.dispatch(1, vec![event("oc_slow", "req-slow")]);
        scheduler.dispatch(2, vec![event("oc_fast", "req-fast")]);

        // 后一批先完成，但确认前缀不能被推进到慢车道之前。
        wait_for_completed(&scheduler, 1, Duration::from_secs(5));
        assert_eq!(scheduler.take_completion().acked_offset, 0);

        release.send(()).expect("释放慢车道");
        assert!(
            wait_for_acked(&scheduler, 2, Duration::from_secs(5)),
            "慢车道完成后确认前缀应连续推进"
        );
    }

    #[test]
    fn whole_batch_of_malformed_lines_still_advances_offsets() {
        let (scheduler, _wake) = scheduler(|_input| Ok(executed()));

        // 整批都是坏行：事件为空，但读偏移与确认前缀必须继续前进。
        scheduler.dispatch(5, Vec::new());

        assert_eq!(scheduler.read_offset(), 5);
        assert_eq!(scheduler.take_completion().acked_offset, 5);
    }

    #[test]
    fn failed_task_is_retried_until_it_succeeds() {
        let attempts = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&attempts);
        let (scheduler, _wake) = scheduler(move |_input| {
            let mut attempts = counter.lock().unwrap_or_else(PoisonError::into_inner);
            *attempts += 1;
            if *attempts < 2 {
                return Err("瞬时失败".to_owned());
            }
            Ok(executed())
        });

        scheduler.dispatch(1, vec![event("oc_a", "req-1")]);

        assert!(
            wait_for_acked(&scheduler, 1, Duration::from_secs(10)),
            "失败任务必须在退避后重试并最终确认"
        );
        assert_eq!(*attempts.lock().unwrap_or_else(PoisonError::into_inner), 2);
    }

    #[test]
    fn read_window_stops_accepting_batches_until_ack_advances() {
        let (release, released) = mpsc::channel::<()>();
        let held = Arc::new(Mutex::new(Some(released)));
        let slow_holder = Arc::clone(&held);
        let (scheduler, _wake) = scheduler(move |input| {
            if input.request_id == "req-slow" {
                let receiver = slow_holder
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take()
                    .expect("只持有一条慢车道");
                let _ = receiver.recv_timeout(Duration::from_secs(5));
            }
            Ok(executed())
        });

        // 第 0 行永久占用一条车道，后续行即使全部完成也不能推进确认前缀。
        scheduler.dispatch(1, vec![event("oc_slow", "req-slow")]);
        assert!(scheduler.can_accept_batch());

        let tail_start = 1u64;
        let tail_end = GATEWAY_LANE_MAX_UNACKED_LINES;
        let tail_events = (tail_start..tail_end)
            .map(|line| event("oc_fast", &format!("req-{line}")))
            .collect::<Vec<_>>();
        scheduler.dispatch(tail_end, tail_events);

        assert_eq!(scheduler.take_completion().acked_offset, 0);
        assert!(
            !scheduler.can_accept_batch(),
            "未确认行达到上界后必须停止读取"
        );

        release.send(()).expect("释放车道");
        assert!(
            wait_for_acked(&scheduler, tail_end, Duration::from_secs(5)),
            "慢车道完成后应重新允许读取"
        );
        assert!(scheduler.can_accept_batch());
    }

    fn wait_for_acked(scheduler: &GatewayLaneScheduler, expected: u64, timeout: Duration) -> bool {
        wait_until(timeout, || {
            scheduler.take_completion().acked_offset >= expected
        })
    }

    fn wait_for_completed(
        scheduler: &GatewayLaneScheduler,
        expected: usize,
        timeout: Duration,
    ) -> bool {
        wait_until(timeout, || {
            scheduler.take_completion().completed >= expected
        })
    }

    fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if predicate() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        predicate()
    }
}
