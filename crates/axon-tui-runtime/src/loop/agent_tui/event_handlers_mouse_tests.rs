//! Agent TUI 鼠标语义测试。

use ratatui::{backend::CrosstermBackend, buffer::Buffer, layout::Size};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::r#loop::agent_tui::state::viewport_state::TranscriptViewBoundsCause;
use crate::r#loop::agent_tui::state::visible_text_selection::VisibleTextSnapshot;
use crate::surface::cell::{RenderCellRecord, RenderReasoningCell};
use crate::{r#loop::agent_tui::tui_event::UiEvent, ui_workflow::action_kind::UiActionKind};

use super::{
    AgentTuiRunOutput, AgentTuiRuntimeAdapter, AgentTuiSubmit, AgentTuiSubmitFailure,
    AgentTuiToolKind, RuntimeEventEmitter, event_handlers::TuiEventOutcome,
    event_handlers::dispatch_test_ui_event, run_output::AgentTuiRuntimeEvent, state::AgentTuiState,
    state_tests::launch_request, submission::RuntimeSubmissionDriver,
};
use crate::app_shell::layout::layout_app_shell;
use crate::widgets::transcript_view_metrics;
use axon_domain::ToolExecutionPermission;

type TestAgentTerminal =
    crate::r#loop::agent_tui::custom_terminal::Terminal<CrosstermBackend<Vec<u8>>>;

fn dispatch_mouse(state: &mut AgentTuiState, mouse: MouseEvent) -> TuiEventOutcome {
    let mut terminal = test_terminal();
    let mut driver = test_driver();
    dispatch_test_ui_event(&mut terminal, state, &mut driver, UiEvent::Mouse(mouse))
        .expect("鼠标 UiEvent 调度不应失败")
}

#[test]
fn mapped_action_catalog_covers_primary_interaction_paths() {
    let mut state = state_with_history();
    state.set_frame_size(Size::new(80, 24));
    assert_eq!(
        super::event_handlers::mapped_action_kind_for_test(
            &state,
            UiEvent::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 0,
                row: viewport_test_row(&state),
                modifiers: KeyModifiers::NONE,
            }),
        ),
        Some(UiActionKind::ScrollActiveSurfaceUp)
    );
    assert_eq!(
        super::event_handlers::mapped_action_kind_for_test(
            &state,
            UiEvent::Mouse(MouseEvent {
                kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
                column: 4,
                row: viewport_test_row(&state),
                modifiers: KeyModifiers::NONE,
            }),
        ),
        Some(UiActionKind::VisibleTextSelectionDrag)
    );

    assert_eq!(
        super::event_handlers::mapped_action_kind_for_test(
            &state,
            UiEvent::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )),
        ),
        None
    );
}

#[test]
fn normal_mouse_wheel_scrolls_render_viewport_history() {
    let mut state = state_with_history();
    state.set_frame_size(Size::new(80, 24));
    let row = viewport_test_row(&state);

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(state.viewport.transcript_view_offset_rows > 0);
}

#[test]
fn clicking_tool_cell_toggles_details_and_recomputes_height() {
    let mut state = state_with_runtime_cell();
    state.set_frame_size(Size::new(80, 24));
    let part_id = "tool-part:test:call-1";
    let index = state
        .cells
        .transcript
        .iter()
        .position(|line| line.semantic_part_id().as_deref() == Some(part_id))
        .expect("工具 cell 应存在");
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_output_chunk_event(
            super::run_output::AgentTuiToolOutputChunk {
                part_id: part_id.to_owned(),
                provider_call_id: "call-1".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 2,
                text: (0..20)
                    .map(|line| format!("result-{line}-{}", "x".repeat(100)))
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
        ),
    );

    let layout = layout_app_shell(
        ratatui::layout::Rect::new(0, 0, 80, 24),
        state.composer_height(),
        super::render::header_bar_height(&state, 80),
    );
    let (collapsed_height, screen_row) = transcript_cell_position(&state, index);

    click_transcript_row(&mut state, screen_row);
    assert_eq!(state.viewport.selected_runtime_part_id(), Some(part_id));
    let expanded_height = transcript_cell_height(&state, index, layout.viewport);
    assert!(
        expanded_height > collapsed_height,
        "collapsed={collapsed_height}, expanded={expanded_height}"
    );

    click_transcript_row(&mut state, screen_row);
    assert_eq!(state.viewport.selected_runtime_part_id(), None);
    let collapsed_height_after_hydration = transcript_cell_height(&state, index, layout.viewport);
    assert!(
        collapsed_height_after_hydration < expanded_height,
        "收起后必须回到摘要高度：collapsed={collapsed_height_after_hydration}, expanded={expanded_height}"
    );
    assert_eq!(
        transcript_cell_height(&state, index, layout.viewport),
        collapsed_height_after_hydration
    );
}

#[test]
fn clicking_completed_reasoning_toggles_body_visibility() {
    let mut state = state_with_runtime_cell();
    state.set_frame_size(Size::new(80, 24));
    let part_id = "reasoning-part:test:click";
    let index = state.cells.transcript.len();
    state
        .cells
        .transcript
        .push(RenderCellRecord::reasoning_with_data(RenderReasoningCell {
            id: part_id.to_owned(),
            text: "先检查调用链，再比较并发边界".to_owned(),
            done: true,
            elapsed_ms: Some(1_250),
        }));
    let (collapsed_height, screen_row) = transcript_cell_position(&state, index);

    click_transcript_row(&mut state, screen_row);
    assert_eq!(state.viewport.selected_runtime_part_id(), Some(part_id));
    let layout = layout_app_shell(
        ratatui::layout::Rect::new(0, 0, 80, 24),
        state.composer_height(),
        super::render::header_bar_height(&state, 80),
    );
    let expanded_height = transcript_cell_height(&state, index, layout.viewport);
    assert!(expanded_height > collapsed_height);

    click_transcript_row(&mut state, screen_row);
    assert_eq!(state.viewport.selected_runtime_part_id(), None);
    assert_eq!(
        transcript_cell_height(&state, index, layout.viewport),
        collapsed_height
    );
}

fn click_transcript_row(state: &mut AgentTuiState, row: u16) {
    let down = dispatch_mouse(
        state,
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );
    let up = dispatch_mouse(
        state,
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );
    assert_eq!(down, TuiEventOutcome::handled(true));
    assert_eq!(up, TuiEventOutcome::handled(true));
}

fn transcript_cell_position(state: &AgentTuiState, index: usize) -> (usize, u16) {
    let size = state.viewport.frame_size;
    let layout = layout_app_shell(
        ratatui::layout::Rect::new(0, 0, size.width, size.height),
        state.composer_height(),
        super::render::header_bar_height(state, size.width),
    );
    let metrics = transcript_view_metrics(
        &state.cells.transcript,
        layout.viewport.height,
        layout.viewport.width,
        state.selected_runtime_render_index_by_identity(),
        None,
        None,
        &state.viewport.transcript_render_cache,
    );
    let cell = metrics
        .cells
        .iter()
        .find(|cell| cell.transcript_index == Some(index))
        .expect("工具 cell 应有高度指标");
    let top_row = metrics
        .max_offset_rows
        .saturating_sub(state.viewport.transcript_view_offset_rows);
    (
        cell.height,
        layout.viewport.y + u16::try_from(cell.top - top_row).unwrap(),
    )
}

fn transcript_cell_height(
    state: &AgentTuiState,
    index: usize,
    area: ratatui::layout::Rect,
) -> usize {
    transcript_view_metrics(
        &state.cells.transcript,
        area.height,
        area.width,
        state.selected_runtime_render_index_by_identity(),
        None,
        None,
        &state.viewport.transcript_render_cache,
    )
    .cells
    .iter()
    .find(|cell| cell.transcript_index == Some(index))
    .expect("工具 cell 应有高度指标")
    .height
}

#[test]
fn mouse_wheel_over_composer_scrolls_transcript_globally() {
    let mut state = state_with_history();
    state.set_frame_size(Size::new(80, 24));
    let composer_row = state.viewport.frame_size.height.saturating_sub(1);

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: composer_row,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(state.viewport.transcript_view_offset_rows > 0);
    assert_eq!(state.composer.draft.text(), "");
}

#[test]
fn mouse_wheel_over_header_scrolls_transcript_globally() {
    let mut state = state_with_history();
    state.set_frame_size(Size::new(80, 24));

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
    assert!(state.viewport.transcript_view_offset_rows > 0);
}

#[test]
fn unsupported_mouse_event_does_not_request_redraw() {
    let mut state = state_with_history();

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::Ignored);
}

#[test]
fn viewport_left_mouse_down_anchors_visible_text_selection_without_cell_selection() {
    let mut state = state_with_runtime_cell();
    state.set_frame_size(Size::new(80, 24));
    let row = viewport_test_row(&state);

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(state.visible_text_selection.active_range().is_none());
    assert_eq!(state.viewport.selected_runtime_render_index(), None);

    let drag = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: 4,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(drag, TuiEventOutcome::handled(true));
    assert!(state.visible_text_selection.active_range().is_some());
    assert_eq!(state.viewport.selected_runtime_render_index(), None);
}

#[test]
fn viewport_right_click_is_ignored_by_visible_text_selection_model() {
    let mut state = state_with_runtime_cell();
    state.set_frame_size(Size::new(80, 24));
    let row = viewport_test_row(&state);

    let outcome = dispatch_mouse(
        &mut state,
        MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Right),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::Ignored);
    assert_eq!(state.viewport.selected_runtime_render_index(), None);
}

fn visible_text_snapshot(lines: &[&str]) -> VisibleTextSnapshot {
    let buffer = Buffer::with_lines(lines.iter().copied());
    VisibleTextSnapshot::from_buffer(&buffer)
}

#[test]
fn visible_area_drag_release_copies_header_viewport_and_composer_text() {
    let mut state = state_with_runtime_cell();
    state
        .visible_text_selection
        .update_snapshot(visible_text_snapshot(&[
            "Header 模型 gpt",
            "渲染区输出",
            "输入区草稿",
        ]));
    let transcript_len_before = state.cells.transcript.len();
    let mut terminal = test_terminal();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter);

    let _ = dispatch_test_ui_event(
        &mut terminal,
        &mut state,
        &mut driver,
        UiEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: 7,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }),
    )
    .expect("mouse dispatch should succeed");
    let _ = dispatch_test_ui_event(
        &mut terminal,
        &mut state,
        &mut driver,
        UiEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Drag(crossterm::event::MouseButton::Left),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }),
    )
    .expect("mouse dispatch should succeed");
    let outcome = dispatch_test_ui_event(
        &mut terminal,
        &mut state,
        &mut driver,
        UiEvent::Mouse(MouseEvent {
            kind: MouseEventKind::Up(crossterm::event::MouseButton::Left),
            column: 2,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }),
    )
    .expect("mouse dispatch should succeed");

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.cells.transcript.len(), transcript_len_before);
    let mut copied = String::new();
    state
        .visible_text_selection
        .update_snapshot(visible_text_snapshot(&[
            "Header 模型 gpt",
            "渲染区输出",
            "输入区草稿",
        ]));
    state.visible_text_selection.begin(0, 7);
    assert!(state.visible_text_selection.update(2, 2));
    assert!(state.copy_visible_text_selection_with(|text| {
        copied = text.to_owned();
        Ok(())
    }));
    assert_eq!(copied, "模型 gpt\n渲染区输出\n输入");
    let toast = state
        .current_toast(std::time::Instant::now())
        .expect("复制成功后应显示 toast");
    assert_eq!(toast.text, "已复制 15 个字符");

    let expired_at = std::time::Instant::now() + std::time::Duration::from_secs(3);
    assert!(state.tick(expired_at));
    assert!(state.current_toast(expired_at).is_none());
}

#[test]
fn visible_area_copy_after_conversation_output_strips_terminal_hyperlinks() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state
        .cells
        .transcript
        .replace_with_records(vec![RenderCellRecord::agent_message(
            "assistant:test:link",
            "查看 https://example.com/docs 获取详情",
        )]);
    state.replace_draft("下一轮输入".to_owned());
    let mut terminal = crate::r#loop::agent_tui::custom_terminal::Terminal::new_for_test(
        ratatui::backend::TestBackend::new(100, 12),
        12,
    )
    .expect("测试终端应可创建");

    terminal
        .draw(|frame| super::render::render_tui(frame, &mut state))
        .expect("TUI 渲染不应失败");
    state
        .visible_text_selection
        .update_snapshot(terminal.visible_text_snapshot());

    let mut copied = String::new();
    state.visible_text_selection.begin(0, 0);
    assert!(state.visible_text_selection.update(11, 99));
    assert!(state.copy_visible_text_selection_with(|text| {
        copied = text.to_owned();
        Ok(())
    }));

    assert!(copied.contains("https://example.com/docs"));
    assert!(!copied.contains("\u{1b}]8"));
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

fn test_driver() -> RuntimeSubmissionDriver<TestRuntimeAdapter> {
    RuntimeSubmissionDriver::new(TestRuntimeAdapter)
}

fn test_terminal() -> TestAgentTerminal {
    crate::r#loop::agent_tui::custom_terminal::Terminal::new_for_test(
        CrosstermBackend::new(Vec::new()),
        4,
    )
    .expect("测试终端应可创建")
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

fn state_with_runtime_cell() -> AgentTuiState {
    let mut state = state_with_history();
    append_exec_cell_with_output(&mut state, "call-1", "cargo check");
    state
}

fn viewport_test_row(state: &AgentTuiState) -> u16 {
    let size = state.viewport.frame_size;
    let layout = layout_app_shell(
        ratatui::layout::Rect::new(0, 0, size.width, size.height),
        state.composer_height(),
        super::render::header_bar_height(state, size.width),
    );
    layout.viewport.y
}

fn append_exec_cell_with_output(state: &mut AgentTuiState, call_id: &str, tool_name: &str) {
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            super::run_output::AgentTuiToolExecutionView {
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
            super::run_output::AgentTuiToolOutputChunk {
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
