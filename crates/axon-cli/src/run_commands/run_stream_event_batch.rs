//! Provider 实时文本事件的有界聚合与强制 flush。
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use axon_protocol::wire::{WireEventEnvelope, WireEventKind};
use axon_session_runtime::model::RuntimeEvent;
use axon_session_runtime::{TurnEventSink, wire_event_to_runtime_event};

const TEXT_BATCH_FLUSH_INTERVAL: Duration = Duration::from_millis(12);
const TEXT_BATCH_MAX_BYTES: usize = 16 * 1024;
const TEXT_EVENT_CHANNEL_CAPACITY: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
enum TextBatchKey {
    Assistant(String),
    Reasoning(String),
}

struct PendingTextBatch {
    key: TextBatchKey,
    events: Vec<RuntimeEvent>,
    bytes: usize,
    started_at: Instant,
}

impl PendingTextBatch {
    fn new(key: TextBatchKey, event: RuntimeEvent) -> Self {
        let bytes = event.payload_json.len();
        Self {
            key,
            events: vec![event],
            bytes,
            started_at: Instant::now(),
        }
    }

    fn can_append(&self, key: &TextBatchKey, event: &RuntimeEvent) -> bool {
        self.key == *key
            && self.bytes.saturating_add(event.payload_json.len()) <= TEXT_BATCH_MAX_BYTES
    }

    fn push(&mut self, event: RuntimeEvent) {
        self.bytes = self.bytes.saturating_add(event.payload_json.len());
        self.events.push(event);
    }

    fn remaining(&self) -> Duration {
        TEXT_BATCH_FLUSH_INTERVAL.saturating_sub(self.started_at.elapsed())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextEventClass {
    Assistant,
    Reasoning,
}

impl TextEventClass {
    fn key(self, envelope: &WireEventEnvelope) -> TextBatchKey {
        match (self, &envelope.kind) {
            (Self::Assistant, WireEventKind::AssistantMessage(message)) => {
                TextBatchKey::Assistant(message.part_id.clone())
            }
            (Self::Reasoning, WireEventKind::ReasoningDelta(reasoning)) => {
                TextBatchKey::Reasoning(reasoning.part_id.clone())
            }
            _ => unreachable!("文本事件类别与 wire event 不匹配"),
        }
    }
}

fn text_event_class(envelope: &WireEventEnvelope) -> Option<TextEventClass> {
    match &envelope.kind {
        WireEventKind::AssistantMessage(message)
            if message.delta && !message.content.is_empty() =>
        {
            Some(TextEventClass::Assistant)
        }
        WireEventKind::ReasoningDelta(reasoning) if !reasoning.delta.is_empty() => {
            Some(TextEventClass::Reasoning)
        }
        _ => None,
    }
}

fn is_first_visible_text(class: TextEventClass) -> bool {
    matches!(class, TextEventClass::Assistant | TextEventClass::Reasoning)
}

fn commit_events(sink: &dyn TurnEventSink, events: &mut Vec<RuntimeEvent>) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    sink.publish_batch(events)?;
    events.clear();
    Ok(())
}

fn flush_pending(
    sink: &dyn TurnEventSink,
    pending: &mut Option<PendingTextBatch>,
) -> Result<(), String> {
    if let Some(mut batch) = pending.take() {
        commit_events(sink, &mut batch.events)?;
    }
    Ok(())
}

fn receive_timeout(
    receiver: &Receiver<WireEventEnvelope>,
    pending: &Option<PendingTextBatch>,
) -> Result<WireEventEnvelope, RecvTimeoutError> {
    match pending {
        Some(batch) => receiver.recv_timeout(batch.remaining()),
        None => receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
    }
}

/// 消费一个 turn 的事件。任一 durable 投影失败都会终止 worker，禁止带缺口收口。
pub(super) fn run_event_batch_worker(
    receiver: Receiver<WireEventEnvelope>,
    sink: &dyn TurnEventSink,
) -> Result<(), String> {
    let mut pending = None;
    let mut first_visible_text = false;

    loop {
        match receive_timeout(&receiver, &pending) {
            Ok(envelope) => {
                let runtime_event = wire_event_to_runtime_event(&envelope)?;
                if let Some(class) = text_event_class(&envelope) {
                    let key = class.key(&envelope);
                    if !first_visible_text {
                        flush_pending(sink, &mut pending)?;
                        let mut events = vec![runtime_event];
                        commit_events(sink, &mut events)?;
                        first_visible_text = is_first_visible_text(class);
                    } else if pending
                        .as_ref()
                        .is_some_and(|batch| batch.can_append(&key, &runtime_event))
                    {
                        pending
                            .as_mut()
                            .expect("pending batch checked")
                            .push(runtime_event);
                        if pending
                            .as_ref()
                            .is_some_and(|batch| batch.bytes >= TEXT_BATCH_MAX_BYTES)
                        {
                            flush_pending(sink, &mut pending)?;
                        }
                    } else {
                        flush_pending(sink, &mut pending)?;
                        pending = Some(PendingTextBatch::new(key, runtime_event));
                    }
                } else {
                    flush_pending(sink, &mut pending)?;
                    let mut events = vec![runtime_event];
                    commit_events(sink, &mut events)?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                flush_pending(sink, &mut pending)?;
            }
            Err(RecvTimeoutError::Disconnected) => {
                flush_pending(sink, &mut pending)?;
                return Ok(());
            }
        }
    }
}

pub(super) const fn channel_capacity() -> usize {
    TEXT_EVENT_CHANNEL_CAPACITY
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::wire::{
        WireAssistantMessage, WireCorrelationId, WireReasoningDelta, WireSchemaVersion,
        WireSessionRef,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        batches: Mutex<Vec<Vec<String>>>,
    }

    impl TurnEventSink for RecordingSink {
        fn publish(&self, event: &RuntimeEvent) -> Result<(), String> {
            self.publish_batch(std::slice::from_ref(event))
        }

        fn publish_batch(&self, events: &[RuntimeEvent]) -> Result<(), String> {
            self.batches.lock().expect("记录批次锁").push(
                events
                    .iter()
                    .map(|event| event.payload_json.clone())
                    .collect(),
            );
            Ok(())
        }
    }

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

    fn assistant(sequence: u64, part_id: &str, content: &str) -> WireEventEnvelope {
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

    fn reasoning(sequence: u64, part_id: &str, delta: &str) -> WireEventEnvelope {
        envelope(
            sequence,
            WireEventKind::ReasoningDelta(WireReasoningDelta {
                part_id: part_id.to_owned(),
                reasoning_id: "reasoning".to_owned(),
                delta: delta.to_owned(),
            }),
        )
    }

    fn assistant_final(sequence: u64, part_id: &str) -> WireEventEnvelope {
        envelope(
            sequence,
            WireEventKind::AssistantMessage(WireAssistantMessage {
                part_id: part_id.to_owned(),
                message_id: "message".to_owned(),
                content: "完成".to_owned(),
                delta: false,
            }),
        )
    }

    struct FailingSink;

    impl TurnEventSink for FailingSink {
        fn publish(&self, _event: &RuntimeEvent) -> Result<(), String> {
            Err("测试提交失败".to_owned())
        }
    }

    #[test]
    fn first_delta_is_immediate_and_following_same_part_is_batched() {
        let sink = RecordingSink::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(assistant(1, "part", "a")).expect("首帧");
        sender.send(assistant(2, "part", "b")).expect("第二帧");
        sender.send(assistant(3, "part", "c")).expect("第三帧");
        drop(sender);

        run_event_batch_worker(receiver, &sink).expect("批处理应成功");
        let batches = sink.batches.lock().expect("读取批次");
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].len(), 1);
        assert_eq!(batches[1].len(), 2);
    }

    #[test]
    fn type_and_part_changes_flush_before_next_text() {
        let sink = RecordingSink::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(assistant(1, "a", "a")).expect("首帧");
        sender.send(assistant(2, "a", "b")).expect("assistant");
        sender.send(reasoning(3, "r", "r")).expect("reasoning");
        sender.send(assistant(4, "b", "c")).expect("part");
        drop(sender);

        run_event_batch_worker(receiver, &sink).expect("批处理应成功");
        let batches = sink.batches.lock().expect("读取批次");
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![1, 1, 1, 1]
        );
    }

    #[test]
    fn timer_flushes_without_a_following_provider_frame() {
        let sink = RecordingSink::default();
        let (sender, receiver) = std::sync::mpsc::sync_channel(channel_capacity());
        sender.send(assistant(1, "a", "a")).expect("首帧");
        sender.send(assistant(2, "a", "b")).expect("第二帧");
        let result = std::thread::scope(|scope| {
            let worker = scope.spawn(|| run_event_batch_worker(receiver, &sink));
            std::thread::sleep(TEXT_BATCH_FLUSH_INTERVAL + Duration::from_millis(4));
            drop(sender);
            worker.join().expect("worker")
        });
        result.expect("定时 flush 应成功");
    }

    #[test]
    fn one_hundred_deltas_use_bounded_number_of_batches() {
        let sink = RecordingSink::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        for sequence in 1..=100 {
            sender
                .send(assistant(sequence, "part", "片段"))
                .expect("delta");
        }
        drop(sender);

        run_event_batch_worker(receiver, &sink).expect("批处理应成功");
        let batches = sink.batches.lock().expect("读取批次");
        assert!(
            batches.len() <= 30,
            "100 个 delta 未达到至少 70% 批次下降: {}",
            batches.len()
        );
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), 100);
    }

    #[test]
    fn byte_limit_flushes_before_the_next_large_delta() {
        let sink = RecordingSink::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        let large = "x".repeat(TEXT_BATCH_MAX_BYTES);
        sender.send(assistant(1, "part", "a")).expect("首帧");
        sender.send(assistant(2, "part", &large)).expect("大帧");
        sender.send(assistant(3, "part", &large)).expect("大帧");
        drop(sender);

        run_event_batch_worker(receiver, &sink).expect("批处理应成功");
        let batches = sink.batches.lock().expect("读取批次");
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![1, 1, 1]
        );
    }

    #[test]
    fn non_text_event_flushes_pending_text_first() {
        let sink = RecordingSink::default();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(assistant(1, "part", "a")).expect("首帧");
        sender.send(assistant(2, "part", "b")).expect("文本");
        sender.send(assistant_final(3, "part")).expect("终态边界");
        drop(sender);

        run_event_batch_worker(receiver, &sink).expect("批处理应成功");
        let batches = sink.batches.lock().expect("读取批次");
        assert_eq!(
            batches.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![1, 1, 1]
        );
    }

    #[test]
    fn failed_batch_stops_the_projection_worker() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(assistant(1, "part", "a")).expect("首帧");
        drop(sender);

        let error = run_event_batch_worker(receiver, &FailingSink)
            .expect_err("durable 投影失败必须结束 worker");
        assert_eq!(error, "测试提交失败");
    }
}
