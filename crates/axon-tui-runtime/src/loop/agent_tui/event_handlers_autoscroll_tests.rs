//! RenderViewport auto-scroll 防线测试。

use std::time::Duration;

use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Size;

use crate::{
    app_shell::composer_dock::DockKind,
    r#loop::agent_tui::state::viewport_state::{TranscriptViewBoundsCause, ViewportScrollState},
    r#loop::agent_tui::tui_event::UiEvent,
};
use axon_domain::ToolExecutionPermission;

use super::{
    AgentTuiRunOutput, AgentTuiRuntimeAdapter, AgentTuiSubmit, AgentTuiSubmitFailure,
    AgentTuiToolKind, RuntimeEventEmitter,
    custom_terminal::Terminal,
    event_handlers::{TuiEventOutcome, dispatch_test_ui_event},
    run_output::{AgentTuiRuntimeEvent, AgentTuiToolExecutionView, AgentTuiToolOutputChunk},
    state::AgentTuiState,
    state_tests::launch_request,
    submission::RuntimeSubmissionDriver,
};
use crate::surface::cell::RenderCellRecord;
use crate::surface::height_index::LiveTranscriptTail;
use crate::widgets::transcript_view_metrics;

type TestAgentTerminal = Terminal<CrosstermBackend<Vec<u8>>>;

#[test]
fn mouse_wheel_scrolls_main_viewport_history() {
    let mut state = state_with_history();

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.viewport.transcript_view_offset_rows, 3);
    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
}

#[test]
fn main_viewport_user_detached_does_not_follow_new_tail() {
    let mut state = state_with_history();
    state.scroll_transcript_view_up();
    let detached_offset = state.viewport.transcript_view_offset_rows;
    let detached_max = state.viewport.max_transcript_view_offset_rows;

    for index in 0..5 {
        state
            .cells
            .transcript
            .push(RenderCellRecord::system(format!("新历史行 {index}")));
    }
    let max_offset_rows = transcript_view_metrics(
        &state.cells.transcript,
        10,
        80,
        state.viewport.selected_runtime_render_index(),
        None,
        None,
        &state.viewport.transcript_render_cache,
    )
    .max_offset_rows;
    state.viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        max_offset_rows,
        10,
        None,
        None,
    );

    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
    assert_eq!(
        state.viewport.transcript_view_offset_rows,
        detached_offset + max_offset_rows.saturating_sub(detached_max)
    );
}

#[test]
fn streaming_delta_preserves_mouse_detached_viewport_offset() {
    let mut state = state_with_history();

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );
    let detached_offset = state.viewport.transcript_view_offset_rows;
    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(detached_offset > 0);

    state.apply_assistant_delta_batch(
        "assistant:test".to_owned(),
        vec!["### 标题\n正文正在流式生成".to_owned()],
    );
    refresh_bounds_with_live_tail(&mut state);

    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
    assert!(state.viewport.transcript_view_offset_rows >= detached_offset);
}

#[test]
fn mouse_wheel_scrolls_streaming_output_before_next_viewport_reflow() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.set_frame_size(Size::new(80, 12));
    state.cells.transcript.clear();
    let delta = (0..48)
        .map(|index| format!("流式输出行 {index}"))
        .collect::<Vec<_>>()
        .join("\n");

    state.apply_assistant_delta_batch("assistant:test:live".to_owned(), vec![format!("{delta}\n")]);
    let base = std::time::Instant::now();
    for tick in 1..=32 {
        let _ = state.tick(base + Duration::from_millis(tick * 90));
    }
    assert!(
        state
            .cells
            .transcript
            .last()
            .is_some_and(|line| line.text.lines().count() > 10)
    );
    assert_eq!(state.viewport.max_transcript_view_offset_rows, 0);

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(state.viewport.max_transcript_view_offset_rows > 0);
    assert!(state.viewport.transcript_view_offset_rows > 0);
    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::UserDetached
    );
}

#[test]
fn tool_output_mouse_scroll_does_not_detach_transcript_view() {
    let mut state = state_with_history();
    state.viewport.detach_transcript_view_at_offset(0);
    append_exec_cell_with_output(&mut state, "call-nested", "cargo check");
    assert!(state.open_selected_tool_output_dock());

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.active_dock_kind(), Some(DockKind::ToolOutputViewer));
    assert_eq!(
        state.viewport.transcript_view_scroll_state,
        ViewportScrollState::BottomAttached
    );
    assert_eq!(state.viewport.transcript_view_offset_rows, 0);
}

fn state_with_history() -> AgentTuiState {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.cells.transcript.clear();
    for index in 0..32 {
        state
            .cells
            .transcript
            .push(RenderCellRecord::system(format!("历史行 {index}")));
    }
    let max_offset_rows = transcript_view_metrics(
        &state.cells.transcript,
        10,
        80,
        state.viewport.selected_runtime_render_index(),
        None,
        None,
        &state.viewport.transcript_render_cache,
    )
    .max_offset_rows;
    state.viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ContentAppended,
        max_offset_rows,
        10,
        None,
        None,
    );
    state
}

fn refresh_bounds_with_live_tail(state: &mut AgentTuiState) {
    let live_tail = state.live_transcript_tail_lines();
    let active_tail = live_tail.as_ref().map(|(lines, key)| LiveTranscriptTail {
        lines: lines.as_slice(),
        key: *key,
    });
    let max_offset_rows = transcript_view_metrics(
        &state.cells.transcript,
        10,
        80,
        state.viewport.selected_runtime_render_index(),
        None,
        active_tail,
        &state.viewport.transcript_render_cache,
    )
    .max_offset_rows;
    state.viewport.restore_transcript_view_bounds(
        TranscriptViewBoundsCause::ViewportReflow,
        max_offset_rows,
        10,
        None,
        None,
    );
}

fn append_exec_cell_with_output(state: &mut AgentTuiState, call_id: &str, tool_name: &str) {
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: format!("tool-part:test:{call_id}"),
                request_id: 1,
                provider_call_id: call_id.to_owned(),
                tool_name: tool_name.to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                arguments_preview: "{}".to_owned(),
                intent: None,
                execution_permission: ToolExecutionPermission::Allowed,
            },
        ),
    );
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            AgentTuiToolOutputChunk {
                part_id: format!("tool-part:test:{call_id}"),
                provider_call_id: call_id.to_owned(),
                tool_name: tool_name.to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: format!("输出 {call_id}"),
            },
        ),
    );
}

fn test_terminal() -> TestAgentTerminal {
    Terminal::new_for_test(CrosstermBackend::new(Vec::new()), 4).expect("测试终端应可创建")
}

fn dispatch_mouse(state: &mut AgentTuiState, mouse: MouseEvent) -> TuiEventOutcome {
    let mut terminal = test_terminal();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter);
    dispatch_test_ui_event(&mut terminal, state, &mut driver, UiEvent::Mouse(mouse))
        .expect("鼠标事件分发不应失败")
}

#[derive(Clone, Default)]
struct TestRuntimeAdapter;

impl AgentTuiRuntimeAdapter for TestRuntimeAdapter {
    fn submit(
        &self,
        _submission: &AgentTuiSubmit,
        _cancel_token: &super::AgentTuiCancelToken,
        _emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure> {
        Ok(AgentTuiRunOutput::assistant("完成"))
    }
}
