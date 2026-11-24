//! Agent TUI 普通交互事件路由。

#[path = "event_handlers/keyboard.rs"]
mod keyboard;
#[path = "event_handlers/mouse.rs"]
mod mouse;
#[path = "event_handlers/state_reducer.rs"]
mod state_reducer;

use std::{io::Write, time::Instant};

use crossterm::event::{self, Event};
use ratatui::backend::CrosstermBackend;

use super::{
    AgentTerminal, AgentTuiError, AgentTuiInterruptPayload, AgentTuiRuntimeAdapter,
    AgentTuiRuntimePayload, custom_terminal::Terminal, external_editor, runtime_driver_event,
    state::AgentTuiState, submission::RuntimeSubmissionDriver, terminal_session,
};
use crate::{
    r#loop::agent_tui::tui_event::UiEvent,
    ui_workflow::{
        action::UiAction,
        reducer::{UiActionOutcome, UiActionReducer, UiReducer, UiSideEffect},
        shortcuts as workflow_shortcuts,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TuiEventOutcome {
    Ignored,
    Handled { needs_redraw: bool },
    Exit,
}

impl TuiEventOutcome {
    pub(super) const fn handled(needs_redraw: bool) -> Self {
        Self::Handled { needs_redraw }
    }
}

pub(super) fn handle_tui_event<F>(
    terminal: &mut AgentTerminal,
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
) -> Result<TuiEventOutcome, AgentTuiError>
where
    F: AgentTuiRuntimeAdapter,
{
    let event = read_ui_event()?;
    handle_ui_event(terminal, state, runtime_driver, event)
}

fn read_ui_event() -> Result<UiEvent, AgentTuiError> {
    match event::read().map_err(AgentTuiError::Io)? {
        Event::Key(key) => Ok(UiEvent::Key(key)),
        Event::Mouse(mouse) => Ok(UiEvent::Mouse(mouse)),
        Event::Paste(text) => Ok(UiEvent::Paste(text)),
        Event::Resize(width, height) => Ok(UiEvent::Resize { width, height }),
        Event::FocusGained | Event::FocusLost => Ok(UiEvent::Tick {
            now: Instant::now(),
        }),
    }
}

pub(super) fn handle_ui_event<F, W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    event: UiEvent,
) -> Result<TuiEventOutcome, AgentTuiError>
where
    F: AgentTuiRuntimeAdapter,
    W: Write,
{
    if let UiEvent::Resize { width, height } = event {
        state.set_frame_size(ratatui::layout::Size::new(width, height));
        return Ok(TuiEventOutcome::handled(true));
    }
    let Some(action) = map_ui_event_to_action(state, &event) else {
        return Ok(TuiEventOutcome::Ignored);
    };
    dispatch_ui_action(terminal, state, runtime_driver, action)
}

fn dispatch_ui_action<F, W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    action: UiAction,
) -> Result<TuiEventOutcome, AgentTuiError>
where
    F: AgentTuiRuntimeAdapter,
    W: Write,
{
    if action == UiAction::Exit {
        return Ok(TuiEventOutcome::Exit);
    }
    let outcome = UiReducer::reduce(state, action);
    let needs_redraw = outcome.needs_redraw;
    execute_side_effect(terminal, state, runtime_driver, outcome)?;
    if state.should_exit() {
        return Ok(TuiEventOutcome::Exit);
    }
    Ok(TuiEventOutcome::handled(needs_redraw))
}

fn execute_side_effect<F, W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    outcome: UiActionOutcome,
) -> Result<(), AgentTuiError>
where
    F: AgentTuiRuntimeAdapter,
    W: Write,
{
    match outcome.side_effect {
        None => Ok(()),
        Some(UiSideEffect::RuntimeSubmit) => {
            if let Some(submission) = state.take_pending_runtime_submit() {
                runtime_driver.start(submission);
            }
            Ok(())
        }
        Some(UiSideEffect::RuntimeSteer) => {
            runtime_driver.submit_steer(state);
            Ok(())
        }
        Some(UiSideEffect::AnswerQuestion) => {
            runtime_driver.answer_question(state);
            Ok(())
        }
        Some(UiSideEffect::RuntimeInterrupt { reason }) => {
            interrupt_runtime(state, runtime_driver, reason);
            Ok(())
        }
        Some(UiSideEffect::OpenExternalEditor) => open_external_editor(terminal, state),
    }
}

fn interrupt_runtime<F>(
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    reason: &'static str,
) where
    F: AgentTuiRuntimeAdapter,
{
    runtime_driver.invalidate_runtime_events();
    if let Some(cancel_token) = runtime_driver.cancel_token() {
        cancel_token.cancel();
    }
    state.request_interrupt_current_task();
    state.apply_runtime_event(runtime_driver_event(
        AgentTuiRuntimePayload::InterruptRequested(AgentTuiInterruptPayload::new(reason)),
    ));
}

fn map_ui_event_to_action(state: &AgentTuiState, event: &UiEvent) -> Option<UiAction> {
    let action = match event {
        UiEvent::Paste(text) => map_paste_to_action(state, text),
        UiEvent::Mouse(mouse) => mouse::map_mouse_to_action(*mouse),
        UiEvent::Key(key) => keyboard::map_key_to_action(state, *key),
        UiEvent::Resize { .. } | UiEvent::Tick { .. } => Some(UiAction::Noop),
    }?;
    debug_assert!(
        workflow_shortcuts::action_has_catalog_binding(&action),
        "UiAction 缺少 workflow 快捷键映射目录：{action:?}"
    );
    Some(action)
}

fn map_paste_to_action(state: &AgentTuiState, text: &str) -> Option<UiAction> {
    state
        .active_dock_kind()
        .is_none()
        .then(|| UiAction::PasteText(text.to_owned()))
}

#[cfg(test)]
pub(super) fn dispatch_runtime_ui_event_for_test<F>(
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    event: UiEvent,
) -> bool
where
    F: AgentTuiRuntimeAdapter,
{
    if !state.is_working() {
        return false;
    }
    let Some(action) = map_ui_event_to_action(state, &event) else {
        return false;
    };
    if matches!(action, UiAction::OpenExternalEditor) {
        return false;
    }
    let outcome = UiReducer::reduce(state, action);
    execute_runtime_side_effect_without_terminal(state, runtime_driver, &outcome);
    outcome.needs_redraw || outcome.side_effect.is_some()
}

#[cfg(test)]
pub(super) fn dispatch_test_ui_event<F, W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    event: UiEvent,
) -> Result<TuiEventOutcome, AgentTuiError>
where
    F: AgentTuiRuntimeAdapter,
    W: Write,
{
    handle_ui_event(terminal, state, runtime_driver, event)
}

#[cfg(test)]
pub(super) fn mapped_action_kind_for_test(
    state: &AgentTuiState,
    event: UiEvent,
) -> Option<crate::ui_workflow::action_kind::UiActionKind> {
    let action = map_ui_event_to_action(state, &event)?;
    crate::ui_workflow::action_kind::action_kind(&action)
}

#[cfg(test)]
fn execute_runtime_side_effect_without_terminal<F>(
    state: &mut AgentTuiState,
    runtime_driver: &mut RuntimeSubmissionDriver<F>,
    outcome: &UiActionOutcome,
) where
    F: AgentTuiRuntimeAdapter,
{
    match &outcome.side_effect {
        Some(UiSideEffect::RuntimeSubmit) => {
            if let Some(submission) = state.take_pending_runtime_submit() {
                runtime_driver.start(submission);
            }
        }
        Some(UiSideEffect::RuntimeSteer) => {
            runtime_driver.submit_steer(state);
        }
        Some(UiSideEffect::AnswerQuestion) => {
            runtime_driver.answer_question(state);
        }
        Some(UiSideEffect::RuntimeInterrupt { reason }) => {
            interrupt_runtime(state, runtime_driver, reason);
        }
        Some(UiSideEffect::OpenExternalEditor) | None => {}
    }
}

fn open_external_editor<W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    state: &mut AgentTuiState,
) -> Result<(), AgentTuiError>
where
    W: Write,
{
    let initial = state.composer.draft.text().to_owned();
    if let Some(value) = edit_text_with_external_editor(terminal, state, &initial)? {
        state.apply_external_edit(value);
    }
    Ok(())
}

fn edit_text_with_external_editor<W>(
    terminal: &mut Terminal<CrosstermBackend<W>>,
    state: &mut AgentTuiState,
    initial: &str,
) -> Result<Option<String>, AgentTuiError>
where
    W: Write,
{
    terminal_session::suspend_for_external_editor(terminal)?;
    let edited = external_editor::edit_draft(initial);
    terminal_session::resume_after_external_editor(terminal)?;

    match edited {
        Ok(value) => Ok(Some(value)),
        Err(error) => {
            state.push_editor_error(&external_editor::format_editor_error(&error));
            Ok(None)
        }
    }
}
