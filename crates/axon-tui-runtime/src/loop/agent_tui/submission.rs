#[cfg(test)]
use std::sync::{mpsc, mpsc::TryRecvError};
use std::{
    any::Any,
    panic, thread,
    time::{Duration, Instant},
};

use super::{
    AgentTuiCancelToken, AgentTuiError, AgentTuiRunOutput, AgentTuiRuntimeAdapter,
    AgentTuiRuntimeEvent, AgentTuiRuntimePayload, AgentTuiSteerResult, AgentTuiSubmitFailure,
    RuntimeError, RuntimeEventEmitter,
    notification::TuiNotificationManager,
    runtime_driver_event,
    state::{AgentTuiState, AgentTuiSubmit},
};
use crate::surface::event_envelope::RuntimeEventId;
use crate::surface::mailbox_telemetry::MailboxTelemetry;
use crate::surface::runtime_adapter::RuntimeEventSender;
use axon_agent_runtime::ProjectionRuntimeBudget;
#[cfg(test)]
use crossterm::event::Event;

#[path = "submission_answer.rs"]
mod submission_answer;
mod submission_delta;
#[path = "submission_wire_order.rs"]
mod submission_wire_order;
mod submission_worker_mailbox;

use submission_delta::{AssistantDeltaFrame, PendingAssistantDeltaBatch};
use submission_worker_mailbox::{SubmissionOutcome, WorkerMailbox};

const WORKER_MESSAGE_DRAIN_BUDGET: usize = 16;
const RUNTIME_IDLE_NOTICE_AFTER: Duration = Duration::from_mins(5);
const ASSISTANT_DELTA_MERGE_WINDOW: Duration = Duration::from_millis(16);

type WorkerMessageDrain = (Option<SubmissionOutcome>, bool, bool);
pub(super) struct RuntimeSubmissionDriver<F>
where
    F: AgentTuiRuntimeAdapter,
{
    submit: F,
    active: Option<ActiveSubmission>,
    notifications: TuiNotificationManager,
    mailbox_telemetry: MailboxTelemetry,
}

struct ActiveSubmission {
    mailbox: WorkerMailbox,
    worker_handles: Vec<thread::JoinHandle<()>>,
    cancel_token: AgentTuiCancelToken,
    last_runtime_event_id: Option<RuntimeEventId>,
    pending_assistant_delta: PendingAssistantDeltaBatch,
    pending_assistant_delta_started_at: Option<Instant>,
    last_activity_at: Instant,
    idle_notice_shown: bool,
}

impl<F> RuntimeSubmissionDriver<F>
where
    F: AgentTuiRuntimeAdapter,
{
    pub(super) fn new(submit: F) -> Self {
        let mailbox_telemetry = MailboxTelemetry::from_config();
        Self {
            submit,
            active: None,
            notifications: TuiNotificationManager::default(),
            mailbox_telemetry,
        }
    }

    pub(super) fn mailbox_telemetry(&self) -> MailboxTelemetry {
        self.mailbox_telemetry.clone()
    }

    /// 当前是否存在活跃提交；完成通知的自动继续据此决定「接上」还是「另起一轮」。
    pub(super) fn is_active(&self) -> bool {
        self.active.is_some()
    }

    #[cfg(test)]
    pub(super) fn set_last_activity_for_test(&mut self, last_activity_at: Instant) {
        if let Some(active) = self.active.as_mut() {
            active.last_activity_at = last_activity_at;
        }
    }

    pub(super) fn start(&mut self, submission: AgentTuiSubmit) {
        self.start_worker(submission, None);
    }

    pub(super) fn start_resume(
        &mut self,
        submission: AgentTuiSubmit,
        active_turn: crate::r#loop::agent_tui::AgentTuiActiveTurn,
        request: crate::r#loop::agent_tui::AgentTuiLaunchRequest,
    ) {
        self.start_worker(submission, Some((active_turn, request)));
    }

    fn start_worker(
        &mut self,
        submission: AgentTuiSubmit,
        resume: Option<(
            crate::r#loop::agent_tui::AgentTuiActiveTurn,
            crate::r#loop::agent_tui::AgentTuiLaunchRequest,
        )>,
    ) {
        let mailbox = WorkerMailbox::with_telemetry(self.mailbox_telemetry.clone());
        let cancel_token = AgentTuiCancelToken::default();
        let worker_cancel_token = cancel_token.clone();
        let adapter = self.submit.clone();
        let started_at = Instant::now();
        self.notifications.start_turn(started_at);
        let worker_mailbox = mailbox.clone();
        let try_emit_mailbox = mailbox.clone();
        let worker_handle = thread::spawn(move || {
            let response = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                let mut emit_progress = |event| {
                    worker_mailbox.push_progress(event);
                };
                let runtime_sender = RuntimeEventSender::new(move |event| {
                    try_emit_mailbox.push_progress(event);
                    Ok(())
                });
                let mut emitter =
                    RuntimeEventEmitter::with_try_emit(&mut emit_progress, runtime_sender);
                match resume {
                    Some((active_turn, request)) => adapter
                        .resume_active_turn(
                            &active_turn,
                            &request,
                            &worker_cancel_token,
                            &mut emitter,
                        )?
                        .ok_or_else(|| {
                            AgentTuiSubmitFailure::without_output(RuntimeError::runtime(
                                "Daemon 活动 Interaction 无法恢复",
                            ))
                        }),
                    None => adapter.submit(&submission, &worker_cancel_token, &mut emitter),
                }
            }))
            .unwrap_or_else(|panic_payload| {
                Err(AgentTuiSubmitFailure::without_output(
                    RuntimeError::worker_panic(panic_summary(panic_payload)),
                ))
            });
            worker_cancel_token.settle();
            worker_mailbox.finish(response);
        });
        self.active = Some(ActiveSubmission {
            mailbox,
            worker_handles: vec![worker_handle],
            cancel_token,
            last_runtime_event_id: None,
            pending_assistant_delta: PendingAssistantDeltaBatch::default(),
            pending_assistant_delta_started_at: None,
            last_activity_at: started_at,
            idle_notice_shown: false,
        });
    }

    pub(super) fn cancel_token(&self) -> Option<AgentTuiCancelToken> {
        self.active
            .as_ref()
            .map(|active| active.cancel_token.clone())
    }

    pub(super) fn invalidate_runtime_events(&mut self) {
        if let Some(active) = self.active.as_ref() {
            active.mailbox.invalidate();
        }
    }

    pub(super) fn submit_steer(&mut self, state: &mut AgentTuiState) -> bool {
        let Some((submission, history_record)) = state.take_steer_submission_from_queue() else {
            return false;
        };
        if !self.submit.contract().capabilities.steer {
            state.return_steer_to_queue(submission, history_record);
            state.push_steer_unsupported_notice();
            return true;
        }
        let Some(active) = self.active.as_ref() else {
            state.return_steer_to_queue(submission, history_record);
            state.push_steer_rejected_notice(Some(RuntimeError::runtime(
                "runtime steer channel unavailable",
            )));
            return true;
        };
        let adapter = self.submit.clone();
        let mailbox = active.mailbox.clone();
        let steer_handle = thread::spawn(move || {
            let response = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                let emit_mailbox = mailbox.clone();
                let mut emit_progress = |event| {
                    emit_mailbox.push_progress(event);
                };
                let try_emit_mailbox = mailbox.clone();
                let runtime_sender = RuntimeEventSender::new(move |event| {
                    try_emit_mailbox.push_progress(event);
                    Ok(())
                });
                let mut emitter =
                    RuntimeEventEmitter::with_try_emit(&mut emit_progress, runtime_sender);
                adapter.steer(&submission, &mut emitter)
            }))
            .unwrap_or_else(|panic_payload| {
                Err(RuntimeError::worker_panic(panic_summary(panic_payload)))
            });
            let apply = move |state: &mut AgentTuiState| match response {
                Ok(AgentTuiSteerResult::Accepted) => {
                    state.record_pending_steer(submission, history_record);
                }
                Ok(AgentTuiSteerResult::Unsupported) => {
                    state.return_steer_to_queue(submission, history_record);
                    state.push_steer_unsupported_notice();
                }
                Ok(AgentTuiSteerResult::Rejected) => {
                    state.return_steer_to_queue(submission, history_record);
                    state.push_steer_rejected_notice(None);
                }
                Err(error) => {
                    state.return_steer_to_queue(submission, history_record);
                    state.push_steer_rejected_notice(Some(error));
                }
            };
            mailbox.push_apply_steer(Box::new(apply));
        });
        if let Some(active) = self.active.as_mut() {
            active.worker_handles.push(steer_handle);
        }
        true
    }

    /// 把待决问答的作答发往统一答案入口；实现见 `submission_answer` 子模块。
    pub(super) fn answer_question(&mut self, state: &mut AgentTuiState) -> bool {
        submission_answer::answer_question(self, state)
    }

    pub(super) fn tick(&mut self, state: &mut AgentTuiState) -> Result<bool, AgentTuiError> {
        let mut dirty = false;
        let Some(active) = self.active.as_mut() else {
            return Ok(dirty);
        };
        let previous_runtime_event_id = active.last_runtime_event_id;
        let (finished, worker_dirty, _worker_mailbox_drained) = drain_worker_mailbox_with_event_id(
            state,
            &active.mailbox,
            &mut self.notifications,
            &mut active.last_runtime_event_id,
            &mut active.pending_assistant_delta,
            &mut active.pending_assistant_delta_started_at,
            false,
        )?;
        dirty |= worker_dirty;
        if dirty || finished.is_some() || active.last_runtime_event_id != previous_runtime_event_id
        {
            active.last_activity_at = Instant::now();
            active.idle_notice_shown = false;
        }
        let interrupted = state.interrupt_requested();
        if let Some(response) = finished {
            self.notifications
                .notify_finished(&response, Instant::now())
                .map_err(AgentTuiError::Io)?;
            let last_runtime_event_id = active.last_runtime_event_id;
            active.cancel_token.settle();
            self.finish_active_submission();
            state.finish_submit_with_cause(response, last_runtime_event_id);
            self.start_next_queued_submission(state);
            return Ok(true);
        }
        if interrupted {
            active.cancel_token.cancel();
            active.cancel_token.mark_cancel_observed();
            active.cancel_token.settle();
            self.finish_active_submission();
            self.start_next_queued_submission(state);
            return Ok(true);
        }
        let idle_for = active.last_activity_at.elapsed();
        if !active.idle_notice_shown && idle_for >= RUNTIME_IDLE_NOTICE_AFTER {
            state.push_runtime_idle_notice(idle_for);
            active.idle_notice_shown = true;
            return Ok(true);
        }
        Ok(dirty)
    }

    fn start_next_queued_submission(&mut self, state: &mut AgentTuiState) {
        if state.should_exit() || self.active.is_some() {
            return;
        }
        if let Some(submission) = state.take_queued_submit() {
            state.start_queued_submit(&submission);
            self.start(submission);
        }
    }

    fn finish_active_submission(&mut self) {
        let Some(active) = self.active.take() else {
            return;
        };
        active.mailbox.invalidate();
        for handle in active.worker_handles {
            let _ = handle.join();
        }
    }
}

impl<F> Drop for RuntimeSubmissionDriver<F>
where
    F: AgentTuiRuntimeAdapter,
{
    fn drop(&mut self) {
        if let Some(active) = self.active.as_ref() {
            active.cancel_token.cancel();
        }
        self.finish_active_submission();
    }
}

fn panic_summary(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_owned()
}

enum SubmissionWorkerMessage {
    Progress(Box<AgentTuiRuntimeEvent>),
    ApplySteer(Box<dyn FnOnce(&mut AgentTuiState) + Send>),
    #[cfg(test)]
    Finished(SubmissionOutcome),
}

#[cfg(test)]
fn drain_worker_messages(
    state: &mut AgentTuiState,
    rx: &mpsc::Receiver<SubmissionWorkerMessage>,
    notifications: &mut TuiNotificationManager,
) -> Result<(Option<SubmissionOutcome>, bool), AgentTuiError> {
    let mut last_runtime_event_id = None;
    let mut pending_assistant_delta = PendingAssistantDeltaBatch::default();
    let mut pending_assistant_delta_started_at = None;
    drain_worker_messages_with_event_id(
        state,
        rx,
        notifications,
        &mut last_runtime_event_id,
        &mut pending_assistant_delta,
        &mut pending_assistant_delta_started_at,
        true,
    )
    .map(|(finished, dirty, _)| (finished, dirty))
}

#[cfg(test)]
fn drain_worker_messages_with_event_id(
    state: &mut AgentTuiState,
    rx: &mpsc::Receiver<SubmissionWorkerMessage>,
    _notifications: &mut TuiNotificationManager,
    last_runtime_event_id: &mut Option<RuntimeEventId>,
    pending_assistant_delta: &mut PendingAssistantDeltaBatch,
    pending_assistant_delta_started_at: &mut Option<Instant>,
    flush_pending_at_end: bool,
) -> Result<WorkerMessageDrain, AgentTuiError> {
    let mut progress_dirty = false;
    let mut worker_mailbox_drained = false;
    for _ in 0..WORKER_MESSAGE_DRAIN_BUDGET {
        let message = match rx.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => {
                worker_mailbox_drained = true;
                break;
            }
            Err(TryRecvError::Disconnected) => {
                let flushed = flush_pending_assistant_delta(
                    state,
                    pending_assistant_delta,
                    pending_assistant_delta_started_at,
                );
                return Ok((
                    Some(Err(AgentTuiSubmitFailure::without_output(
                        RuntimeError::worker_channel_disconnected(),
                    ))),
                    progress_dirty || flushed,
                    true,
                ));
            }
        };
        match message {
            SubmissionWorkerMessage::Progress(event) => {
                progress_dirty |= apply_worker_progress_event(
                    state,
                    *event,
                    pending_assistant_delta,
                    pending_assistant_delta_started_at,
                    last_runtime_event_id,
                )?;
            }
            SubmissionWorkerMessage::ApplySteer(apply) => {
                flush_pending_assistant_delta(
                    state,
                    pending_assistant_delta,
                    pending_assistant_delta_started_at,
                );
                apply(state);
                progress_dirty = true;
            }
            SubmissionWorkerMessage::Finished(response) => {
                let flushed = flush_pending_assistant_delta(
                    state,
                    pending_assistant_delta,
                    pending_assistant_delta_started_at,
                );
                return Ok((Some(response), progress_dirty || flushed, true));
            }
        }
    }
    if flush_pending_at_end
        || assistant_delta_batch_window_elapsed(*pending_assistant_delta_started_at, Instant::now())
    {
        progress_dirty |= flush_pending_assistant_delta(
            state,
            pending_assistant_delta,
            pending_assistant_delta_started_at,
        );
    }
    Ok((None, progress_dirty, worker_mailbox_drained))
}

fn drain_worker_mailbox_with_event_id(
    state: &mut AgentTuiState,
    mailbox: &WorkerMailbox,
    _notifications: &mut TuiNotificationManager,
    last_runtime_event_id: &mut Option<RuntimeEventId>,
    pending_assistant_delta: &mut PendingAssistantDeltaBatch,
    pending_assistant_delta_started_at: &mut Option<Instant>,
    flush_pending_at_end: bool,
) -> Result<WorkerMessageDrain, AgentTuiError> {
    let mut progress_dirty = false;
    for _ in 0..WORKER_MESSAGE_DRAIN_BUDGET {
        let Some(message) = mailbox.try_take() else {
            let flushed = flush_pending_assistant_delta(
                state,
                pending_assistant_delta,
                pending_assistant_delta_started_at,
            );
            return Ok((
                mailbox.take_finished_when_drained(),
                progress_dirty || flushed,
                true,
            ));
        };
        match message {
            SubmissionWorkerMessage::Progress(event) => {
                progress_dirty |= apply_worker_progress_event(
                    state,
                    *event,
                    pending_assistant_delta,
                    pending_assistant_delta_started_at,
                    last_runtime_event_id,
                )?;
            }
            SubmissionWorkerMessage::ApplySteer(apply) => {
                flush_pending_assistant_delta(
                    state,
                    pending_assistant_delta,
                    pending_assistant_delta_started_at,
                );
                apply(state);
                progress_dirty = true;
            }
            #[cfg(test)]
            SubmissionWorkerMessage::Finished(_) => {
                unreachable!("终态仅能写入 WorkerMailbox 的独占槽")
            }
        }
    }
    if flush_pending_at_end
        || assistant_delta_batch_window_elapsed(*pending_assistant_delta_started_at, Instant::now())
    {
        progress_dirty |= flush_pending_assistant_delta(
            state,
            pending_assistant_delta,
            pending_assistant_delta_started_at,
        );
    }
    Ok((None, progress_dirty, false))
}

fn apply_worker_progress_event(
    state: &mut AgentTuiState,
    event: AgentTuiRuntimeEvent,
    pending_assistant_delta: &mut PendingAssistantDeltaBatch,
    pending_assistant_delta_started_at: &mut Option<Instant>,
    last_runtime_event_id: &mut Option<RuntimeEventId>,
) -> Result<bool, AgentTuiError> {
    *last_runtime_event_id = Some(event.event_id);
    if let Some(mut frame) =
        assistant_delta_from_runtime_payload(&event.payload, event.occurred_at.as_millis())
    {
        frame.transport_timing = event.transport_timing.clone();
        let flushed_before_push = if pending_assistant_delta.has_different_part_id(&frame.part_id) {
            flush_pending_assistant_delta(
                state,
                pending_assistant_delta,
                pending_assistant_delta_started_at,
            )
        } else {
            false
        };
        if pending_assistant_delta_started_at.is_none() {
            *pending_assistant_delta_started_at = Some(Instant::now());
        }
        pending_assistant_delta.push(frame);
        let flushed = if pending_assistant_delta.first_delta_pending()
            || assistant_delta_batch_should_flush(
                &pending_assistant_delta.deltas,
                *pending_assistant_delta_started_at,
                Instant::now(),
            ) {
            flush_pending_assistant_delta(
                state,
                pending_assistant_delta,
                pending_assistant_delta_started_at,
            )
        } else {
            false
        };
        return Ok(flushed_before_push || flushed);
    }
    flush_pending_assistant_delta(
        state,
        pending_assistant_delta,
        pending_assistant_delta_started_at,
    );
    state.apply_runtime_event(event);
    Ok(true)
}

fn flush_pending_assistant_delta(
    state: &mut AgentTuiState,
    pending_assistant_delta: &mut PendingAssistantDeltaBatch,
    pending_assistant_delta_started_at: &mut Option<Instant>,
) -> bool {
    let Some((part_id, deltas, occurred_at_ms, transport_timing)) = pending_assistant_delta.take()
    else {
        return false;
    };
    *pending_assistant_delta_started_at = None;
    let mut event =
        runtime_driver_event(AgentTuiRuntimePayload::BatchedAssistantDelta { part_id, deltas });
    event.occurred_at =
        crate::surface::event_envelope::RuntimeEventTimestamp::from_millis(occurred_at_ms);
    event.transport_timing = transport_timing;
    state.apply_runtime_event(event);
    true
}

fn assistant_delta_from_runtime_payload(
    payload: &AgentTuiRuntimePayload,
    occurred_at_ms: u128,
) -> Option<AssistantDeltaFrame> {
    match payload {
        AgentTuiRuntimePayload::WireEvent { event, .. } => match &event.kind {
            axon_protocol::wire::WireEventKind::AssistantMessage(message) if message.delta => {
                Some(AssistantDeltaFrame {
                    part_id: message.part_id.clone(),
                    content: message.content.clone(),
                    occurred_at_ms: u128::try_from(event.recorded_at_ms).unwrap_or(occurred_at_ms),
                    transport_timing: None,
                })
            }
            _ => None,
        },
        _ => None,
    }
}

fn assistant_delta_batch_should_flush(
    pending_assistant_delta: &[String],
    started_at: Option<Instant>,
    now: Instant,
) -> bool {
    let budget = ProjectionRuntimeBudget::current();
    let total_bytes = pending_assistant_delta
        .iter()
        .map(String::len)
        .fold(0usize, usize::saturating_add);
    pending_assistant_delta.len() >= budget.delta_chunk_count
        || total_bytes >= budget.coalesced_text_bytes
        || pending_assistant_delta
            .iter()
            .any(|delta| delta.contains('\n'))
        || assistant_delta_batch_window_elapsed(started_at, now)
}

fn assistant_delta_batch_window_elapsed(started_at: Option<Instant>, now: Instant) -> bool {
    started_at.is_some_and(|started_at| {
        now.saturating_duration_since(started_at) >= ASSISTANT_DELTA_MERGE_WINDOW
    })
}

#[cfg(test)]
pub(super) fn dispatch_runtime_ui_event_for_test(
    state: &mut AgentTuiState,
    event: &Event,
    cancel_token: &AgentTuiCancelToken,
    runtime_driver: &mut RuntimeSubmissionDriver<impl AgentTuiRuntimeAdapter>,
) -> bool {
    let _ = cancel_token;
    let ui_event = match event {
        Event::Paste(text) => crate::r#loop::agent_tui::tui_event::UiEvent::Paste(text.clone()),
        Event::Key(key) => crate::r#loop::agent_tui::tui_event::UiEvent::Key(*key),
        Event::Mouse(mouse) => crate::r#loop::agent_tui::tui_event::UiEvent::Mouse(*mouse),
        Event::Resize(width, height) => crate::r#loop::agent_tui::tui_event::UiEvent::Resize {
            width: *width,
            height: *height,
        },
        Event::FocusGained | Event::FocusLost => {
            crate::r#loop::agent_tui::tui_event::UiEvent::Tick {
                now: Instant::now(),
            }
        }
    };
    super::event_handlers::dispatch_runtime_ui_event_for_test(state, runtime_driver, ui_event)
}

#[cfg(test)]
#[path = "submission_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "submission_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "submission_provider_error_tests.rs"]
mod provider_error_tests;

#[cfg(test)]
#[path = "submission_queue_tests.rs"]
mod queue_tests;

#[cfg(test)]
#[path = "submission_resilience_tests.rs"]
mod resilience_tests;

#[cfg(test)]
#[path = "completion_notice_resume_tests.rs"]
mod completion_notice_resume_tests;
