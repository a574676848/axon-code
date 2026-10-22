//! TUI 提交 worker 的单一有序语义邮箱。
//!
//! 所有同步 `emit` 与线程安全的 `try_emit` 都进入同一个提交级队列。
//! 顺序由 Daemon durable stream 保证；邮箱只做四件事，绝不改变语义边界：
//! - 幂等去重：同一 durable `(stream, event_id)` 只投影一次；
//! - 序号单调：每个 stream 拒绝序号不高于已投影水位的迟到帧，不排队、不延迟；
//! - 有界批合并：同一 assistant part 的连续 delta 合并为一个投影片段；
//! - 有界背压：队列满载时只淘汰可重建的展示增量帧（流式 delta），
//!   语义边界帧（工具、生命周期、plan、usage、canonical 正文、审批、steer）
//!   既不淘汰也不阻塞生产者。

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    thread,
    time::Instant,
};

use axon_agent_runtime::ProjectionRuntimeBudget;
use axon_protocol::wire::WireEventKind;

use super::super::runtime_bridge::wire_stream_key;
use super::{
    AgentTuiRunOutput, AgentTuiRuntimeEvent, AgentTuiRuntimePayload, AgentTuiSubmitFailure,
    SubmissionWorkerMessage, assistant_delta_from_runtime_payload,
    submission_wire_order::{OrderedWireEvent, WireAdmission, WireStreamGate},
};
use crate::r#loop::agent_tui::state::AgentTuiState;
#[cfg(test)]
use crate::surface::mailbox_telemetry::MailboxTelemetrySnapshot;
use crate::surface::mailbox_telemetry::{MailboxLane, MailboxTelemetry};

type SteerApply = Box<dyn FnOnce(&mut AgentTuiState) + Send>;

/// durable wire 事件的投影身份：stream、事件 ID 与该 stream 内的序号。
struct WireIdentity {
    stream: String,
    event_id: String,
    sequence: u64,
}

/// 提交 worker 的终态投影。
///
/// 失败同样携带宿主已获得的 durable 终态投影：实时邮箱只淘汰可重建的展示增量，
/// 被淘汰的增量只能由终态按 durable 顺序完整重放补齐。
pub(super) type SubmissionOutcome = Result<AgentTuiRunOutput, AgentTuiSubmitFailure>;

#[derive(Clone)]
pub(super) struct WorkerMailbox {
    shared: Arc<WorkerMailboxShared>,
    budget: ProjectionRuntimeBudget,
    telemetry: MailboxTelemetry,
    main_loop_thread: thread::Thread,
}

struct WorkerMailboxShared {
    state: Mutex<WorkerMailboxState>,
}

struct WorkerMailboxState {
    messages: VecDeque<TimedWorkerMessage>,
    sheddable_frames: usize,
    wire_gate: WireStreamGate,
    finished: Option<SubmissionOutcome>,
    invalidated: bool,
}

struct TimedWorkerMessage {
    message: SubmissionWorkerMessage,
    enqueued_at: Option<Instant>,
}

impl WorkerMailbox {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self::with_telemetry(MailboxTelemetry::from_config())
    }

    pub(super) fn with_telemetry(telemetry: MailboxTelemetry) -> Self {
        let budget = ProjectionRuntimeBudget::current();
        Self {
            shared: Arc::new(WorkerMailboxShared {
                state: Mutex::new(WorkerMailboxState {
                    messages: VecDeque::with_capacity(budget.semantic_mailbox_slots),
                    sheddable_frames: 0,
                    wire_gate: WireStreamGate::default(),
                    finished: None,
                    invalidated: false,
                }),
            }),
            budget,
            telemetry,
            main_loop_thread: thread::current(),
        }
    }

    /// 接纳一个 runtime 事件；wire 事件通过身份去重与序号单调闸门后立即进入投影队列。
    ///
    /// 该路径永不阻塞生产者：durable 顺序已经在持久化发布边界确定，
    /// 展示层跟不上时只允许淘汰可重建的展示增量帧。
    pub(super) fn push_progress(&self, event: AgentTuiRuntimeEvent) {
        let enqueued_at = self.telemetry.enqueued_at();
        let mut state = self.lock();
        if state.invalidated {
            return;
        }
        match admit_event(&mut state.wire_gate, event, enqueued_at) {
            WireAdmission::Duplicate => {
                self.telemetry.record_coalesced(MailboxLane::Worker);
                drop(state);
            }
            WireAdmission::Stale => {
                self.telemetry.record_stale(MailboxLane::Worker);
                drop(state);
            }
            WireAdmission::Deliver(ordered) => self.enqueue_ordered(state, ordered),
        }
        self.main_loop_thread.unpark();
    }

    /// 接纳一个 steer 回执：语义边界，永不淘汰、永不阻塞。
    pub(super) fn push_apply_steer(&self, apply: SteerApply) {
        let enqueued_at = self.telemetry.enqueued_at();
        let mut state = self.lock();
        if state.invalidated {
            return;
        }
        state.messages.push_back(TimedWorkerMessage {
            message: SubmissionWorkerMessage::ApplySteer(apply),
            enqueued_at,
        });
        self.telemetry
            .record_enqueued(MailboxLane::Worker, state.messages.len());
        drop(state);
        self.main_loop_thread.unpark();
    }

    pub(super) fn finish(&self, response: SubmissionOutcome) {
        let mut state = self.lock();
        state.finished = Some(response);
        drop(state);
        self.main_loop_thread.unpark();
    }

    pub(super) fn try_take(&self) -> Option<SubmissionWorkerMessage> {
        let mut state = self.lock();
        let timed = state.messages.pop_front()?;
        if is_sheddable_frame(&timed.message) {
            state.sheddable_frames = state.sheddable_frames.saturating_sub(1);
        }
        self.telemetry
            .record_drained(MailboxLane::Worker, timed.enqueued_at);
        Some(timed.message)
    }

    pub(super) fn take_finished_when_drained(&self) -> Option<SubmissionOutcome> {
        let mut state = self.lock();
        state
            .messages
            .is_empty()
            .then(|| state.finished.take())
            .flatten()
    }

    /// 中断后使当前提交的所有后续事件失效，避免旧流污染下一轮提交。
    pub(super) fn invalidate(&self) {
        let mut state = self.lock();
        state.invalidated = true;
        state.messages.clear();
        state.sheddable_frames = 0;
        state.wire_gate = WireStreamGate::default();
        drop(state);
        self.main_loop_thread.unpark();
    }

    /// 展示增量帧允许占用的槽位；其余槽位预留给语义边界帧。
    fn progress_slots(&self) -> usize {
        self.budget
            .semantic_mailbox_slots
            .saturating_sub(self.budget.critical_mailbox_reserve)
    }

    fn enqueue_ordered<'a>(
        &self,
        mut state: std::sync::MutexGuard<'a, WorkerMailboxState>,
        ordered: OrderedWireEvent,
    ) {
        let Some(event) =
            try_coalesce_worker_assistant_delta(&mut state.messages, ordered.event, self.budget)
        else {
            self.telemetry.record_coalesced(MailboxLane::Worker);
            return;
        };
        let sheddable = is_sheddable_event(&event);
        if sheddable {
            while state.sheddable_frames >= self.progress_slots() {
                if !shed_oldest_sheddable_frame(&mut state.messages) {
                    break;
                }
                state.sheddable_frames = state.sheddable_frames.saturating_sub(1);
                self.telemetry.record_shed(MailboxLane::Worker);
            }
            state.sheddable_frames = state.sheddable_frames.saturating_add(1);
        }
        state.messages.push_back(TimedWorkerMessage {
            message: SubmissionWorkerMessage::Progress(Box::new(event)),
            enqueued_at: ordered.enqueued_at,
        });
        self.telemetry
            .record_enqueued(MailboxLane::Worker, state.messages.len());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkerMailboxState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(super) fn telemetry_snapshot(&self) -> MailboxTelemetrySnapshot {
        self.telemetry.snapshot()
    }
}

fn admit_event(
    gate: &mut WireStreamGate,
    event: AgentTuiRuntimeEvent,
    enqueued_at: Option<Instant>,
) -> WireAdmission {
    let Some(identity) = wire_event_identity(&event) else {
        return WireAdmission::Deliver(OrderedWireEvent { event, enqueued_at });
    };
    gate.admit(
        &identity.stream,
        &identity.event_id,
        identity.sequence,
        OrderedWireEvent { event, enqueued_at },
    )
}

fn wire_event_identity(event: &AgentTuiRuntimeEvent) -> Option<WireIdentity> {
    let AgentTuiRuntimePayload::WireEvent { event, .. } = &event.payload else {
        return None;
    };
    Some(WireIdentity {
        stream: wire_stream_key(&event.session.session_id, event.session.run_id.as_deref()),
        event_id: event.event_id.0.clone(),
        sequence: event.sequence,
    })
}

/// 判断帧是否只承载可重建的展示增量。
///
/// 这些事实在终态投影中会被完整重放（canonical 正文快照与 reasoning 全文），
/// 因此过载时淘汰它们不会造成 durable 语义缺口。
fn is_sheddable_frame(message: &SubmissionWorkerMessage) -> bool {
    match message {
        SubmissionWorkerMessage::Progress(event) => is_sheddable_event(event),
        SubmissionWorkerMessage::ApplySteer(_) => false,
        #[cfg(test)]
        SubmissionWorkerMessage::Finished(_) => false,
    }
}

fn is_sheddable_event(event: &AgentTuiRuntimeEvent) -> bool {
    match &event.payload {
        AgentTuiRuntimePayload::BatchedAssistantDelta { .. }
        | AgentTuiRuntimePayload::BatchedReasoningDelta { .. } => true,
        AgentTuiRuntimePayload::WireEvent { event, .. } => {
            matches!(&event.kind, WireEventKind::AssistantMessage(message) if message.delta)
                || matches!(&event.kind, WireEventKind::ReasoningDelta(_))
        }
        _ => false,
    }
}

fn shed_oldest_sheddable_frame(messages: &mut VecDeque<TimedWorkerMessage>) -> bool {
    let Some(index) = messages
        .iter()
        .position(|timed| is_sheddable_frame(&timed.message))
    else {
        return false;
    };
    messages.remove(index);
    true
}

fn try_coalesce_worker_assistant_delta(
    messages: &mut VecDeque<TimedWorkerMessage>,
    event: AgentTuiRuntimeEvent,
    budget: ProjectionRuntimeBudget,
) -> Option<AgentTuiRuntimeEvent> {
    let Some(frame) =
        assistant_delta_from_runtime_payload(&event.payload, event.occurred_at.as_millis())
    else {
        return Some(event);
    };
    let Some(TimedWorkerMessage {
        message: SubmissionWorkerMessage::Progress(tail),
        ..
    }) = messages.back_mut()
    else {
        return Some(event);
    };
    (!append_assistant_delta(&mut tail.payload, frame.part_id, frame.content, budget))
        .then_some(event)
}

fn append_assistant_delta(
    payload: &mut AgentTuiRuntimePayload,
    part_id: String,
    content: String,
    budget: ProjectionRuntimeBudget,
) -> bool {
    match payload {
        AgentTuiRuntimePayload::WireEvent { event, .. } if matches!(&event.kind, WireEventKind::AssistantMessage(message) if message.delta && message.part_id == part_id) =>
        {
            let previous = match &event.kind {
                WireEventKind::AssistantMessage(message) => message.content.clone(),
                _ => return false,
            };
            if !assistant_delta_batch_fits(std::iter::once(previous.as_str()), &content, budget) {
                return false;
            }
            *payload = AgentTuiRuntimePayload::BatchedAssistantDelta {
                part_id,
                deltas: vec![previous, content],
            };
            true
        }
        AgentTuiRuntimePayload::BatchedAssistantDelta {
            part_id: current_part_id,
            deltas,
        } if *current_part_id == part_id
            && assistant_delta_batch_fits(deltas.iter().map(String::as_str), &content, budget) =>
        {
            deltas.push(content);
            true
        }
        _ => false,
    }
}

fn assistant_delta_batch_fits<'a>(
    deltas: impl Iterator<Item = &'a str>,
    content: &str,
    budget: ProjectionRuntimeBudget,
) -> bool {
    let mut count = 1usize;
    let mut bytes = content.len();
    for delta in deltas {
        count = count.saturating_add(1);
        bytes = bytes.saturating_add(delta.len());
    }
    count <= budget.delta_chunk_count && bytes <= budget.coalesced_text_bytes
}
