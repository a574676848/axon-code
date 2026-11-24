//! TUI 帧率调度 - FrameScheduler 语义调度入口。
//!
//! 设计目标：
//! - 调用方只能通过 `FrameScheduler::request_frame()` 发出重绘请求（非阻塞、幂等）。
//! - 请求通道和信号通道容量固定为 1，任意 burst 在主循环消费前只保留一帧。
//! - 目标刷新率为 120Hz，最小帧间隔约 8.33ms；60Hz 终端自然落在该上限内。
//! - 主循环通过 `FrameScheduler::try_take_frame()` 消费绘制信号。
//! - 底层 channel 仅作为模块私有实现细节，避免调用方绕过调度与 telemetry 绑定。

use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

const FRAME_TARGET_HZ: u64 = 120;
const FRAME_REQUEST_CHANNEL_CAPACITY: usize = 1;
const FRAME_SIGNAL_CHANNEL_CAPACITY: usize = 1;
const MIN_FRAME_INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / FRAME_TARGET_HZ);

/// 帧请求原因，主循环所有调度点必须显式归类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameRequestReason {
    InitialMount,
    UiAction,
    RuntimeProgress,
    StateTick,
    PasteBurstFlush,
}

/// 私有帧请求通道。所有请求共享同一个合并线程。
#[derive(Debug, Clone)]
struct FrameRequestChannel {
    tx: SyncSender<()>,
    pending_frame: Arc<AtomicBool>,
}

/// 主循环侧帧调度器，统一绑定帧请求与 telemetry 计数。
#[derive(Debug)]
pub(crate) struct FrameScheduler {
    request_channel: FrameRequestChannel,
    signal_channel: FrameSignalChannel,
}

impl FrameScheduler {
    pub(crate) fn start() -> io::Result<Self> {
        let (request_channel, signal_channel) = FrameRequestChannel::start(thread::current())?;
        Ok(Self {
            request_channel,
            signal_channel,
        })
    }

    pub(crate) fn request_frame(
        &self,
        _reason: FrameRequestReason,
        telemetry: &mut impl FrameScheduleTelemetry,
    ) {
        telemetry.record_frame_scheduled();
        match self.request_channel.request() {
            FrameRequestStatus::Queued => {}
            FrameRequestStatus::Coalesced => telemetry.record_frame_coalesced(1),
            FrameRequestStatus::Closed => telemetry.record_frame_disconnected(),
        }
    }

    pub(crate) fn try_take_frame(&self, telemetry: &mut impl FrameScheduleTelemetry) -> bool {
        match self.signal_channel.take_ready_frame() {
            FrameSignalStatus::Ready => {
                telemetry.record_frame_emitted();
                true
            }
            FrameSignalStatus::Idle => false,
            FrameSignalStatus::Disconnected => {
                telemetry.record_frame_disconnected();
                false
            }
        }
    }
}

pub(crate) trait FrameScheduleTelemetry {
    fn record_frame_scheduled(&mut self);
    fn record_frame_emitted(&mut self);
    fn record_frame_coalesced(&mut self, count: u64);
    fn record_frame_disconnected(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameRequestStatus {
    Queued,
    Coalesced,
    Closed,
}

impl FrameRequestChannel {
    /// 创建帧调度通道并启动后台合并线程。返回请求通道与信号通道。
    fn start(main_loop_thread: thread::Thread) -> io::Result<(Self, FrameSignalChannel)> {
        let (raw_tx, raw_rx) = mpsc::sync_channel::<()>(FRAME_REQUEST_CHANNEL_CAPACITY);
        let (coalesce_tx, coalesce_rx) = mpsc::sync_channel::<()>(FRAME_SIGNAL_CHANNEL_CAPACITY);
        let pending_frame = Arc::new(AtomicBool::new(false));
        let last_emit = Arc::new(Mutex::new(None::<Instant>));

        // 后台合并线程：接收请求，按 MIN_FRAME_INTERVAL 节流后转发到 coalesce 通道。
        thread::Builder::new()
            .name("axon-frame-coalescer".to_owned())
            .spawn(move || coalescer_loop(raw_rx, last_emit, coalesce_tx, main_loop_thread))?;

        Ok((
            Self {
                tx: raw_tx,
                pending_frame: Arc::clone(&pending_frame),
            },
            FrameSignalChannel {
                rx: coalesce_rx,
                pending_frame,
            },
        ))
    }

    /// 申请重绘（非阻塞）。
    fn request(&self) -> FrameRequestStatus {
        if self.pending_frame.swap(true, Ordering::AcqRel) {
            return FrameRequestStatus::Coalesced;
        }
        match self.tx.try_send(()) {
            Ok(()) => FrameRequestStatus::Queued,
            Err(TrySendError::Full(())) => FrameRequestStatus::Coalesced,
            Err(TrySendError::Disconnected(())) => {
                self.pending_frame.store(false, Ordering::Release);
                FrameRequestStatus::Closed
            }
        }
    }
}

/// 主循环侧的帧信号通道。
#[derive(Debug)]
struct FrameSignalChannel {
    rx: Receiver<()>,
    pending_frame: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameSignalStatus {
    Ready,
    Idle,
    Disconnected,
}

impl FrameSignalChannel {
    /// 非阻塞检查是否有未决绘制请求。
    fn take_ready_frame(&self) -> FrameSignalStatus {
        match self.rx.try_recv() {
            Ok(()) => {
                self.pending_frame.store(false, Ordering::Release);
                FrameSignalStatus::Ready
            }
            Err(TryRecvError::Empty) => FrameSignalStatus::Idle,
            Err(TryRecvError::Disconnected) => {
                self.pending_frame.store(false, Ordering::Release);
                FrameSignalStatus::Disconnected
            }
        }
    }
}

fn coalescer_loop(
    rx: Receiver<()>,
    last_emit: Arc<Mutex<Option<Instant>>>,
    tx: SyncSender<()>,
    main_loop_thread: thread::Thread,
) {
    while rx.recv().is_ok() {
        let deadline = {
            let mut last = last_emit
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let now = Instant::now();
            let earliest = match *last {
                Some(prev) => prev + MIN_FRAME_INTERVAL,
                None => now,
            };
            let deadline = earliest.max(now);
            *last = Some(deadline);
            deadline
        };
        let now = Instant::now();
        if deadline > now {
            thread::sleep(deadline - now);
        }
        if tx.send(()).is_err() {
            return;
        }
        main_loop_thread.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const BURST_REQUEST_COUNT: usize = 1_000;
    const FRAME_SIGNAL_TIMEOUT: Duration = Duration::from_millis(200);
    const FRAME_SIGNAL_POLL_INTERVAL: Duration = Duration::from_millis(2);
    const FRAME_WAKE_TIMEOUT: Duration = Duration::from_secs(1);
    const FRAME_WAKE_LATENCY_LIMIT: Duration = Duration::from_millis(500);

    #[derive(Default)]
    struct TestTelemetry {
        scheduled: usize,
        emitted: usize,
        coalesced: u64,
        disconnected: u64,
    }

    impl FrameScheduleTelemetry for TestTelemetry {
        fn record_frame_scheduled(&mut self) {
            self.scheduled += 1;
        }

        fn record_frame_emitted(&mut self) {
            self.emitted += 1;
        }

        fn record_frame_coalesced(&mut self, count: u64) {
            self.coalesced += count;
        }

        fn record_frame_disconnected(&mut self) {
            self.disconnected += 1;
        }
    }

    #[test]
    fn scheduler_records_request_and_emission_together() {
        let scheduler = FrameScheduler::start().expect("scheduler");
        let mut telemetry = TestTelemetry::default();

        scheduler.request_frame(FrameRequestReason::UiAction, &mut telemetry);

        wait_until_frame(&scheduler, &mut telemetry);
        assert_eq!(telemetry.scheduled, 1);
        assert_eq!(telemetry.emitted, 1);
        assert_eq!(telemetry.coalesced, 0);
    }

    #[test]
    fn scheduler_wakes_owner_thread_when_frame_is_ready() {
        let scheduler = FrameScheduler::start().expect("scheduler");
        let mut telemetry = TestTelemetry::default();
        let started_at = Instant::now();

        scheduler.request_frame(FrameRequestReason::RuntimeProgress, &mut telemetry);
        thread::park_timeout(FRAME_WAKE_TIMEOUT);

        assert!(started_at.elapsed() < FRAME_WAKE_LATENCY_LIMIT);
        assert!(scheduler.try_take_frame(&mut telemetry));
    }

    #[test]
    fn scheduler_collapses_thousand_burst_requests_before_next_tick() {
        let scheduler = FrameScheduler::start().expect("scheduler");
        let mut telemetry = TestTelemetry::default();

        for _ in 0..BURST_REQUEST_COUNT {
            scheduler.request_frame(FrameRequestReason::RuntimeProgress, &mut telemetry);
        }

        wait_until_frame(&scheduler, &mut telemetry);
        assert_eq!(telemetry.scheduled, BURST_REQUEST_COUNT);
        assert_eq!(
            telemetry.coalesced,
            u64::try_from(BURST_REQUEST_COUNT - 1).unwrap()
        );
        assert_eq!(telemetry.emitted, 1);
    }

    #[test]
    fn scheduler_without_request_returns_false() {
        let scheduler = FrameScheduler::start().expect("scheduler");
        let mut telemetry = TestTelemetry::default();

        assert!(!scheduler.try_take_frame(&mut telemetry));
        assert_eq!(telemetry.scheduled, 0);
        assert_eq!(telemetry.emitted, 0);
        assert_eq!(telemetry.coalesced, 0);
    }

    #[test]
    fn disconnected_signal_channel_clears_pending_frame() {
        let (_tx, rx) = mpsc::sync_channel::<()>(FRAME_SIGNAL_CHANNEL_CAPACITY);
        let pending_frame = Arc::new(AtomicBool::new(true));
        let signal_channel = FrameSignalChannel {
            rx,
            pending_frame: Arc::clone(&pending_frame),
        };
        drop(_tx);

        assert_eq!(
            signal_channel.take_ready_frame(),
            FrameSignalStatus::Disconnected
        );
        assert!(!pending_frame.load(Ordering::Acquire));
    }

    #[test]
    fn scheduler_records_closed_request_channel_without_stale_pending() {
        let (request_tx, request_rx) = mpsc::sync_channel::<()>(FRAME_REQUEST_CHANNEL_CAPACITY);
        let (_signal_tx, signal_rx) = mpsc::sync_channel::<()>(FRAME_SIGNAL_CHANNEL_CAPACITY);
        let pending_frame = Arc::new(AtomicBool::new(false));
        let scheduler = FrameScheduler {
            request_channel: FrameRequestChannel {
                tx: request_tx,
                pending_frame: Arc::clone(&pending_frame),
            },
            signal_channel: FrameSignalChannel {
                rx: signal_rx,
                pending_frame: Arc::clone(&pending_frame),
            },
        };
        let mut telemetry = TestTelemetry::default();
        drop(request_rx);

        scheduler.request_frame(FrameRequestReason::UiAction, &mut telemetry);

        assert_eq!(telemetry.scheduled, 1);
        assert_eq!(telemetry.disconnected, 1);
        assert!(!pending_frame.load(Ordering::Acquire));
    }

    fn wait_until_frame(scheduler: &FrameScheduler, telemetry: &mut TestTelemetry) {
        let deadline = Instant::now() + FRAME_SIGNAL_TIMEOUT;
        while Instant::now() < deadline {
            if scheduler.try_take_frame(telemetry) {
                return;
            }
            thread::sleep(FRAME_SIGNAL_POLL_INTERVAL);
        }
        panic!("帧调度器未在 200ms 内发出信号");
    }
}
