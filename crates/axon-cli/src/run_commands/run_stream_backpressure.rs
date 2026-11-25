//! 流式帧通道的非阻塞发送策略与溢出计数。
//!
//! 帧通道的接收端按 12ms / 16KiB 聚合后落盘，落盘变慢时不能反压持有通道的 Provider
//! 线程（同一进程的多个会话共享这个 runtime）。因此发送端按帧的语义分成两类：
//! 正文增量帧只做非阻塞尝试，满时并入暂存帧继续合并，合并不了就丢弃；终态帧与工具
//! 边界帧必须送达，通道满时退化为阻塞发送。
//!
//! 阻塞只落在关键帧上：等待容量的是生产线程，持有容量的是独立的事件消费 worker，
//! worker 只跟随落盘进度前进，不会反过来等待生产线程；worker 退出后通道断开，阻塞的
//! 发送立即返回错误，因此不会死锁。暂存帧未清空之前不发送任何后续帧，同一 part 的
//! 正文帧顺序始终单调。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use axon_protocol::wire::{WireEventEnvelope, WireEventKind};

/// 接收端已退出时返回给调用方的说明。
const FRAME_CHANNEL_CLOSED: &str = "帧通道接收端已退出";
/// 暂存位互斥锁的取锁失败说明。
const PENDING_LOCK: &str = "流式帧暂存锁";

/// 帧通道发送侧计数：收口时一次性写进日志，测试可直接读取。
#[derive(Debug, Default)]
pub(super) struct StreamFrameCounters {
    sent: AtomicU64,
    merged: AtomicU64,
    dropped: AtomicU64,
    forced_blocked: AtomicU64,
}

impl StreamFrameCounters {
    fn note_sent(&self) {
        self.sent.fetch_add(1, Ordering::Relaxed);
    }

    fn note_merged(&self) {
        self.merged.fetch_add(1, Ordering::Relaxed);
    }

    fn note_dropped(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn note_forced_blocked(&self) {
        self.forced_blocked.fetch_add(1, Ordering::Relaxed);
    }

    /// 成功进入通道的帧数，合并帧按发出算一帧。
    pub(super) fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// 被并入暂存帧的正文增量帧数。
    pub(super) fn merged(&self) -> u64 {
        self.merged.load(Ordering::Relaxed)
    }

    /// 因通道满被丢弃的正文增量帧数。
    pub(super) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 关键帧为等容量而阻塞的次数。
    pub(super) fn forced_blocked(&self) -> u64 {
        self.forced_blocked.load(Ordering::Relaxed)
    }

    /// 收口时输出一次计数：全程无溢出与合并走 debug，发生过溢出走 warning。
    pub(super) fn report(&self, session_id: &str) {
        let merged = self.merged();
        let dropped = self.dropped();
        let forced_blocked = self.forced_blocked();
        let summary = format!(
            "流式帧通道计数: session={session_id} 发送成功={} 合并={} 溢出丢弃={} 强制阻塞={}",
            self.sent(),
            merged,
            dropped,
            forced_blocked
        );
        if merged == 0 && dropped == 0 && forced_blocked == 0 {
            axon_logger::debug(&summary);
        } else {
            axon_logger::warning(&summary);
        }
    }
}

/// 正文增量帧：内容只是增量文本，满时可合并、可丢弃。
fn is_coalescible(kind: &WireEventKind) -> bool {
    match kind {
        WireEventKind::AssistantMessage(message) => message.delta,
        WireEventKind::ReasoningDelta(_) => true,
        _ => false,
    }
}

/// 正文增量帧携带的文本长度。
fn delta_len(kind: &WireEventKind) -> usize {
    match kind {
        WireEventKind::AssistantMessage(message) => message.content.len(),
        WireEventKind::ReasoningDelta(reasoning) => reasoning.delta.len(),
        _ => 0,
    }
}

/// 通道满时滞留的正文增量帧。
struct PendingFrame {
    event: WireEventEnvelope,
}

/// 暂存帧的体积上限，与消费端 16KiB 聚合上限同量级：落盘长时间卡死时也不让暂存正文无界增长。
const PENDING_MAX_BYTES: usize = 16 * 1024;

impl PendingFrame {
    /// 后继增量是否属于同一 part，可以并进这条暂存帧。
    ///
    /// 身份同时比对 part 与消息（reasoning 为块）ID，跨消息的增量一律不合并。
    fn same_part(&self, kind: &WireEventKind) -> bool {
        match (&self.event.kind, kind) {
            (WireEventKind::AssistantMessage(left), WireEventKind::AssistantMessage(right)) => {
                right.delta && left.part_id == right.part_id && left.message_id == right.message_id
            }
            (WireEventKind::ReasoningDelta(left), WireEventKind::ReasoningDelta(right)) => {
                left.part_id == right.part_id && left.reasoning_id == right.reasoning_id
            }
            _ => false,
        }
    }

    /// 同一 part 且并入后不超过体积上限，才允许继续合并。
    fn can_merge(&self, kind: &WireEventKind) -> bool {
        self.same_part(kind)
            && delta_len(&self.event.kind).saturating_add(delta_len(kind)) <= PENDING_MAX_BYTES
    }

    /// 把后继增量并入暂存帧，调用方已用 `same_part` 确认过身份。
    ///
    /// 下游把 delta 帧的文本按追加处理，终态帧按整段替换，所以直接拼接增量不改变语义。
    fn merge(&mut self, kind: &WireEventKind) {
        match (&mut self.event.kind, kind) {
            (WireEventKind::AssistantMessage(target), WireEventKind::AssistantMessage(source)) => {
                target.content.push_str(&source.content);
            }
            (WireEventKind::ReasoningDelta(target), WireEventKind::ReasoningDelta(source)) => {
                target.delta.push_str(&source.delta);
            }
            _ => {}
        }
    }
}

/// 有界帧通道的非阻塞发送端。
pub(super) struct StreamFrameSender {
    sender: SyncSender<WireEventEnvelope>,
    counters: Arc<StreamFrameCounters>,
    /// 通道满时滞留的正文增量帧；清空之前不发送任何后续帧，保证发送顺序与到达顺序一致。
    pending: Mutex<Option<PendingFrame>>,
}

impl StreamFrameSender {
    /// 用一条有界帧通道与共享计数构造发送端。
    pub(super) fn new(
        sender: SyncSender<WireEventEnvelope>,
        counters: Arc<StreamFrameCounters>,
    ) -> Self {
        Self {
            sender,
            counters,
            pending: Mutex::new(None),
        }
    }

    /// 发送一帧。
    ///
    /// 正文增量帧永不阻塞：通道满时先暂存，同 part 的后继增量并入暂存帧，换了 part 就
    /// 丢掉旧暂存帧。关键帧（终态、工具边界等）只在通道满时阻塞等容量，等的是消费者
    /// 排空，因此有界且不会死锁。
    pub(super) fn send(&self, event: WireEventEnvelope) -> Result<(), String> {
        if is_coalescible(&event.kind) {
            self.send_coalescible(event)
        } else {
            self.send_critical(event)
        }
    }

    /// 收口补发暂存帧。收口时消费端本就要排空整条通道，这里允许阻塞。
    pub(super) fn finish(&self) -> Result<(), String> {
        let Some(frame) = self.pending.lock().expect(PENDING_LOCK).take() else {
            return Ok(());
        };
        match self.sender.try_send(frame.event) {
            Ok(()) => {
                self.counters.note_sent();
                Ok(())
            }
            Err(TrySendError::Full(event)) => {
                self.counters.note_forced_blocked();
                match self.sender.send(event) {
                    Ok(()) => {
                        self.counters.note_sent();
                        Ok(())
                    }
                    Err(_) => Err(FRAME_CHANNEL_CLOSED.to_owned()),
                }
            }
            Err(TrySendError::Disconnected(_)) => Err(FRAME_CHANNEL_CLOSED.to_owned()),
        }
    }

    /// 正文增量帧的发送：先推暂存，再合并或丢弃，全程不阻塞。
    fn send_coalescible(&self, event: WireEventEnvelope) -> Result<(), String> {
        {
            let mut pending = self.pending.lock().expect(PENDING_LOCK);
            if let Some(frame) = pending.take() {
                match self.sender.try_send(frame.event) {
                    Ok(()) => self.counters.note_sent(),
                    Err(TrySendError::Full(frame)) => {
                        pending.replace(PendingFrame { event: frame });
                    }
                    Err(TrySendError::Disconnected(_)) => {
                        return Err(FRAME_CHANNEL_CLOSED.to_owned());
                    }
                }
            }
            if let Some(frame) = pending.as_mut()
                && frame.can_merge(&event.kind)
            {
                frame.merge(&event.kind);
                self.counters.note_merged();
                return Ok(());
            }
            if pending.is_some() {
                // 通道仍然满，且新帧换了 part 或暂存帧已到体积上限：丢弃旧暂存帧，换成
                // 最新的正文。正文被丢部分由带整段内容的终态帧补齐，顺序不会倒退。
                self.counters.note_dropped();
                pending.take();
            }
        }
        match self.sender.try_send(event) {
            Ok(()) => {
                self.counters.note_sent();
                Ok(())
            }
            Err(TrySendError::Full(event)) => {
                let mut pending = self.pending.lock().expect(PENDING_LOCK);
                // 并发发送端可能刚占住暂存位：保留最新正文，旧帧按丢弃计数。
                if pending.replace(PendingFrame { event }).is_some() {
                    self.counters.note_dropped();
                }
                Ok(())
            }
            Err(TrySendError::Disconnected(_)) => Err(FRAME_CHANNEL_CLOSED.to_owned()),
        }
    }

    /// 关键帧的发送：不得越过更早的暂存帧，通道满时阻塞等容量。
    fn send_critical(&self, event: WireEventEnvelope) -> Result<(), String> {
        let stale = self.pending.lock().expect(PENDING_LOCK).take();
        if let Some(frame) = stale {
            match self.sender.try_send(frame.event) {
                Ok(()) => self.counters.note_sent(),
                Err(TrySendError::Full(_)) => self.counters.note_dropped(),
                Err(TrySendError::Disconnected(_)) => {
                    return Err(FRAME_CHANNEL_CLOSED.to_owned());
                }
            }
        }
        match self.sender.try_send(event) {
            Ok(()) => {
                self.counters.note_sent();
                Ok(())
            }
            Err(TrySendError::Full(event)) => {
                self.counters.note_forced_blocked();
                match self.sender.send(event) {
                    Ok(()) => {
                        self.counters.note_sent();
                        Ok(())
                    }
                    Err(_) => Err(FRAME_CHANNEL_CLOSED.to_owned()),
                }
            }
            Err(TrySendError::Disconnected(_)) => Err(FRAME_CHANNEL_CLOSED.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, sync_channel};
    use std::time::{Duration, Instant};

    use axon_protocol::wire::{
        WireAssistantMessage, WireCorrelationId, WireSchemaVersion, WireSessionRef,
    };

    fn envelope(sequence: u64, kind: WireEventKind) -> WireEventEnvelope {
        WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("event:{sequence}")),
            correlation_id: WireCorrelationId("turn".to_owned()),
            session: WireSessionRef {
                session_id: "session".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: 1,
            kind,
        }
    }

    fn assistant_delta(sequence: u64, part_id: &str, content: &str) -> WireEventEnvelope {
        envelope(
            sequence,
            WireEventKind::AssistantMessage(WireAssistantMessage {
                part_id: part_id.to_owned(),
                message_id: "message".to_owned(),
                content: content.to_owned(),
                delta: true,
            }),
        )
    }

    fn assistant_terminal(sequence: u64, part_id: &str, content: &str) -> WireEventEnvelope {
        envelope(
            sequence,
            WireEventKind::AssistantMessage(WireAssistantMessage {
                part_id: part_id.to_owned(),
                message_id: "message".to_owned(),
                content: content.to_owned(),
                delta: false,
            }),
        )
    }

    fn sender(
        capacity: usize,
    ) -> (
        Arc<StreamFrameSender>,
        Receiver<WireEventEnvelope>,
        Arc<StreamFrameCounters>,
    ) {
        let (channel_sender, receiver) = sync_channel(capacity);
        let counters = Arc::new(StreamFrameCounters::default());
        let frames = Arc::new(StreamFrameSender::new(
            channel_sender,
            Arc::clone(&counters),
        ));
        (frames, receiver, counters)
    }

    fn content_of(event: &WireEventEnvelope) -> &str {
        match &event.kind {
            WireEventKind::AssistantMessage(message) => &message.content,
            _ => panic!("用例只构造 assistant 帧"),
        }
    }

    #[test]
    fn saturated_channel_does_not_block_text_frames() {
        let (frames, receiver, counters) = sender(2);
        frames.send(assistant_delta(1, "part", "甲")).expect("首帧");
        frames
            .send(assistant_delta(2, "part", "乙"))
            .expect("第二帧");

        let started = Instant::now();
        frames
            .send(assistant_delta(3, "part", "丙"))
            .expect("满时暂存");
        frames
            .send(assistant_delta(4, "part", "丁"))
            .expect("满时合并");
        frames
            .send(assistant_delta(5, "other", "戊"))
            .expect("满时换 part");
        let elapsed = started.elapsed();

        assert!(
            elapsed < Duration::from_millis(100),
            "通道满时正文增量发送不得阻塞: {elapsed:?}"
        );
        assert_eq!(counters.sent(), 2);
        assert_eq!(counters.merged(), 1);
        assert_eq!(counters.dropped(), 1);
        assert_eq!(counters.forced_blocked(), 0);
        assert_eq!(content_of(&receiver.recv().expect("首帧")), "甲");
        assert_eq!(content_of(&receiver.recv().expect("第二帧")), "乙");
    }

    #[test]
    fn merged_text_frame_keeps_order_and_full_content() {
        let (frames, receiver, counters) = sender(1);
        frames.send(assistant_delta(1, "part", "甲")).expect("首帧");
        frames
            .send(assistant_delta(2, "part", "乙"))
            .expect("满时暂存");
        frames
            .send(assistant_delta(3, "part", "丙"))
            .expect("满时合并");
        assert_eq!(counters.merged(), 1);

        let first = receiver.recv().expect("首帧");
        frames
            .send(assistant_delta(4, "part", "丁"))
            .expect("推进合并帧");
        let merged = receiver.recv().expect("合并帧");
        assert_eq!(first.sequence, 1);
        assert_eq!(merged.sequence, 2, "合并帧沿用组内首帧序号");
        assert_eq!(content_of(&merged), "乙丙", "同 part 增量按到达顺序拼接");

        frames.finish().expect("收口补发");
        let last = receiver.recv().expect("收口帧");
        assert_eq!(last.sequence, 4);
        assert_eq!(content_of(&last), "丁");
        assert_eq!(counters.sent(), 3);
    }

    #[test]
    fn oversized_pending_text_is_dropped_instead_of_growing() {
        let (frames, receiver, counters) = sender(1);
        let large = "x".repeat(PENDING_MAX_BYTES);
        frames.send(assistant_delta(1, "part", "甲")).expect("首帧");
        frames
            .send(assistant_delta(2, "part", &large))
            .expect("满时暂存");
        frames
            .send(assistant_delta(3, "part", "乙"))
            .expect("超出体积上限");

        assert_eq!(counters.merged(), 0, "超过体积上限不再合并");
        assert_eq!(counters.dropped(), 1, "旧暂存帧按丢弃计数");
        assert_eq!(counters.sent(), 1);
        assert_eq!(receiver.recv().expect("首帧").sequence, 1);
        frames.finish().expect("收口补发");
        let flushed = receiver.recv().expect("收口帧");
        assert_eq!(flushed.sequence, 3);
        assert_eq!(content_of(&flushed), "乙", "暂存位只留最新一帧");
    }

    #[test]
    fn terminal_frame_is_delivered_after_waiting_for_capacity() {
        let (frames, receiver, counters) = sender(1);
        frames
            .send(assistant_delta(1, "part", "正文"))
            .expect("首帧");
        frames
            .send(assistant_delta(2, "part", "尾部"))
            .expect("满时暂存");
        let drain = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let first = receiver.recv().expect("首帧");
            let terminal = receiver.recv().expect("终态帧");
            (first, terminal)
        });

        frames
            .send(assistant_terminal(3, "part", "完整回答"))
            .expect("终态帧必须送达");

        let (first, terminal) = drain.join().expect("排空线程");
        assert_eq!(first.sequence, 1);
        assert_eq!(terminal.sequence, 3);
        assert_eq!(content_of(&terminal), "完整回答");
        assert_eq!(counters.forced_blocked(), 1, "满通道上的关键帧阻塞一次");
        assert_eq!(counters.dropped(), 1, "被关键帧挤掉的暂存帧单独计数");
    }

    #[test]
    fn finish_flushes_pending_frame_when_channel_is_full() {
        let (frames, receiver, counters) = sender(1);
        frames.send(assistant_delta(1, "part", "甲")).expect("首帧");
        frames
            .send(assistant_delta(2, "part", "乙"))
            .expect("满时暂存");
        let drain = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            let first = receiver.recv().expect("首帧");
            let flushed = receiver.recv().expect("收口补发帧");
            (first, flushed)
        });

        frames.finish().expect("收口补发");

        let (first, flushed) = drain.join().expect("排空线程");
        assert_eq!(first.sequence, 1);
        assert_eq!(flushed.sequence, 2);
        assert_eq!(content_of(&flushed), "乙");
        assert_eq!(counters.forced_blocked(), 1);
    }
}
