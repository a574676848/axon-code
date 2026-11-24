//! Agent TUI 事件处理回归测试。

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Size;

use crate::r#loop::agent_tui::state::viewport_state::TranscriptViewBoundsCause;
use crate::r#loop::agent_tui::tui_event::UiEvent;
use crate::ui_workflow::action::UiAction;
use crate::ui_workflow::action_kind::UiActionKind;
use crate::ui_workflow::reducer::{UiActionReducer, UiReducer};
use axon_domain::ToolExecutionPermission;

use super::{
    AgentTuiCatalog, AgentTuiRunOutput, AgentTuiRuntimeAdapter, AgentTuiSubmit,
    AgentTuiSubmitFailure, AgentTuiToolKind, RuntimeEventEmitter,
    custom_terminal::Terminal,
    event_handlers::{TuiEventOutcome, dispatch_test_ui_event, mapped_action_kind_for_test},
    run_output::{AgentTuiRuntimeEvent, AgentTuiToolExecutionView, AgentTuiToolOutputChunk},
    state::AgentTuiState,
    state_tests::launch_request,
    submission::RuntimeSubmissionDriver,
};
use crate::surface::cell::{RenderCellAction, RenderCellRecord};
use crate::surface::cell_event_data::{AgentRenderExecCell, AgentRenderExecCompletion};
use crate::widgets::transcript_view_metrics;

type TestAgentTerminal = Terminal<CrosstermBackend<Vec<u8>>>;

#[test]
fn terminal_resize_requests_full_redraw() {
    let mut state = state_with_history();
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    let outcome = dispatch_test_ui_event(
        &mut terminal,
        &mut state,
        &mut driver,
        UiEvent::Resize {
            width: 120,
            height: 36,
        },
    )
    .expect("resize UiEvent 调度不应失败");

    assert_eq!(outcome, TuiEventOutcome::handled(true));
}

#[test]
fn active_dock_key_router_dispatches_current_dock_mode() {
    let mut state = state_with_history();
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    state.toggle_shortcut_help_dock();
    assert!(state.is_shortcut_help_open());
    assert_eq!(
        dispatch_key(
            &mut terminal,
            &mut state,
            &mut driver,
            KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE),
        ),
        TuiEventOutcome::handled(true)
    );
    assert!(!state.is_shortcut_help_open());
}

#[test]
fn page_keys_scroll_primary_render_viewport_history() {
    let mut state = state_with_history();
    let mut terminal = test_terminal();

    let page_up = dispatch_key_with_default_driver(
        &mut terminal,
        &mut state,
        KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE),
    );

    assert_eq!(page_up, TuiEventOutcome::handled(true));
    assert!(state.viewport.transcript_view_offset_rows > 0);

    let page_down = dispatch_key_with_default_driver(
        &mut terminal,
        &mut state,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    );

    assert_eq!(page_down, TuiEventOutcome::handled(true));
    assert_eq!(state.viewport.transcript_view_offset_rows, 0);
}

#[test]
fn plain_up_recalls_composer_history_without_selecting_runtime_cell() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.cells.transcript.clear();
    state
        .cells
        .transcript
        .push(RenderCellRecord::exec_with_data(
            AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
                id: "exec-part:tool-1".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                status: "completed".to_owned(),
                provider_call_id: "tool-1".to_owned(),
                summary: Some("cargo check 完成".to_owned()),
                arguments_preview: None,
                output_preview: None,
                duration_ms: None,
            }),
            RenderCellAction::Ran,
        ));
    state.submit_plain("历史任务".to_owned());
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    let plain_up = dispatch_key(
        &mut terminal,
        &mut state,
        &mut driver,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
    );

    assert_eq!(plain_up, TuiEventOutcome::handled(true));
    assert_eq!(state.composer.draft.text(), "历史任务");
    assert_eq!(state.viewport.selected_runtime_render_index(), None);

    let alt_up = dispatch_key(
        &mut terminal,
        &mut state,
        &mut driver,
        KeyEvent::new(KeyCode::Up, KeyModifiers::ALT),
    );

    assert_eq!(alt_up, TuiEventOutcome::Ignored);
    assert_eq!(state.viewport.selected_runtime_render_index(), None);
}

#[test]
fn single_line_history_navigation_restores_unsubmitted_draft() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.submit_plain("历史任务".to_owned());
    state.replace_draft("新的未提交输入".to_owned());
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    assert_eq!(
        dispatch_key(
            &mut terminal,
            &mut state,
            &mut driver,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        ),
        TuiEventOutcome::handled(true)
    );
    assert_eq!(state.composer.draft.text(), "历史任务");

    assert_eq!(
        dispatch_key(
            &mut terminal,
            &mut state,
            &mut driver,
            KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        ),
        TuiEventOutcome::handled(true)
    );
    assert_eq!(state.composer.draft.text(), "新的未提交输入");
}

#[test]
fn history_navigation_restores_unsubmitted_attachment_metadata() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.submit_plain("历史任务".to_owned());
    state.composer.draft.attach_file(
        std::path::PathBuf::from("C:/tmp/history-image.png"),
        super::attachments::AgentTuiAttachmentKind::Image,
    );
    let draft_text = state.composer.draft.text().to_owned();

    state.history_previous();
    assert_eq!(state.composer.draft.text(), "历史任务");

    state.history_next();
    assert_eq!(state.composer.draft.text(), draft_text);
    assert_eq!(
        state
            .composer
            .draft
            .input_elements(state.composer.draft.text())
            .len(),
        1
    );
}

#[test]
fn single_visual_line_up_down_uses_history_navigation() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.set_frame_size(Size::new(80, 24));
    state.submit_plain("历史任务".to_owned());
    state.replace_draft("short".to_owned());
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    assert_eq!(
        dispatch_key(
            &mut terminal,
            &mut state,
            &mut driver,
            KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
        ),
        TuiEventOutcome::handled(true)
    );
    assert_eq!(state.composer.draft.text(), "历史任务");
}

#[test]
fn empty_space_stays_text_input_instead_of_selecting_runtime_cell() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.cells.transcript.clear();
    state
        .cells
        .transcript
        .push(RenderCellRecord::exec_with_data(
            AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
                id: "exec-part:tool-1".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                status: "completed".to_owned(),
                provider_call_id: "tool-1".to_owned(),
                summary: Some("cargo check 完成".to_owned()),
                arguments_preview: None,
                output_preview: None,
                duration_ms: None,
            }),
            RenderCellAction::Ran,
        ));
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    let outcome = dispatch_key(
        &mut terminal,
        &mut state,
        &mut driver,
        KeyEvent {
            code: KeyCode::Char(' '),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        },
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.composer.draft.text(), " ");
    assert_eq!(state.viewport.selected_runtime_render_index(), None);
}

#[test]
fn selected_tool_output_follows_part_id_when_cell_index_shifts() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.cells.transcript.clear();
    append_exec_cell_with_output(&mut state, "call-1", "cargo check");
    append_exec_cell_with_output(&mut state, "call-2", "cargo test");
    let selected_index = state
        .cells
        .transcript
        .iter()
        .position(|line| line.part_id() == "tool-part:test:call-2")
        .expect("测试应找到第二个工具 cell");
    state
        .viewport
        .select_runtime_part(selected_index, "tool-part:test:call-2".to_owned());

    let _ = state.cells.transcript.remove(0);

    assert!(state.open_selected_tool_output_dock());
    assert!(
        state
            .viewport
            .tool_output_dock
            .lines()
            .iter()
            .any(|line| { line.contains("输出 call-2") })
    );
    assert!(
        !state
            .viewport
            .tool_output_dock
            .lines()
            .iter()
            .any(|line| { line.contains("输出 call-1") })
    );
}

#[test]
fn tool_output_dock_uses_part_id_when_provider_call_id_is_shared() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.cells.transcript.clear();
    append_exec_cell_with_provider_output(
        &mut state,
        "first",
        "provider-call:shared",
        "cargo check",
        "输出 first",
    );
    append_exec_cell_with_provider_output(
        &mut state,
        "second",
        "provider-call:shared",
        "cargo test",
        "输出 second",
    );
    let selected_index = state
        .cells
        .transcript
        .iter()
        .position(|line| line.part_id() == "tool-part:test:second")
        .expect("测试应找到第二个工具 cell");
    state
        .viewport
        .select_runtime_part(selected_index, "tool-part:test:second".to_owned());

    assert!(state.open_selected_tool_output_dock());
    assert!(
        state
            .viewport
            .tool_output_dock
            .lines()
            .iter()
            .any(|line| { line.contains("输出 second") })
    );
    assert!(
        !state
            .viewport
            .tool_output_dock
            .lines()
            .iter()
            .any(|line| { line.contains("输出 first") })
    );
}

#[test]
fn esc_closes_command_suggestions_without_requesting_tui_exit() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("/mo".to_owned());

    let outcome = dispatch_key_with_default_driver(
        &mut test_terminal(),
        &mut state,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(state.composer.draft.text().is_empty());
    assert!(!state.should_exit());
}

#[test]
fn slash_command_palette_uses_plain_up_down_for_selection() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("/".to_owned());
    let mut terminal = test_terminal();

    assert!(state.command_suggestions_open());
    assert!(!state.composer_popup_open());
    assert_eq!(selected_slash_command(&state), Some("/help"));

    let down = dispatch_key_with_default_driver(
        &mut terminal,
        &mut state,
        KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
    );

    assert_eq!(down, TuiEventOutcome::handled(true));
    assert_eq!(selected_slash_command(&state), Some("/clear"));

    let up = dispatch_key_with_default_driver(
        &mut terminal,
        &mut state,
        KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
    );

    assert_eq!(up, TuiEventOutcome::handled(true));
    assert_eq!(selected_slash_command(&state), Some("/help"));
}

fn selected_slash_command(state: &AgentTuiState) -> Option<&str> {
    state
        .composer
        .command_suggestions
        .get(state.composer.selected_command_suggestion)
        .map(|item| item.command.as_str())
}

#[test]
fn esc_in_input_area_clears_draft_without_exiting_tui() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("继续对齐输入区".to_owned());
    let mut terminal = test_terminal();

    let outcome = dispatch_key_with_default_driver(
        &mut terminal,
        &mut state,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert!(state.composer.draft.text().is_empty());
    assert!(!state.should_exit());
}

#[test]
fn alt_e_restores_queued_message_to_input_area() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("较早任务".to_owned());
    assert!(state.queue_draft_for_next_turn());
    state.replace_draft("最近任务".to_owned());
    assert!(state.queue_draft_for_next_turn());
    let mut terminal = test_terminal();

    let outcome = dispatch_key_with_default_driver(
        &mut terminal,
        &mut state,
        KeyEvent::new(KeyCode::Char('e'), KeyModifiers::ALT),
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.composer.draft.text(), "最近任务");
    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("待发送 1. 较早任务")
    );
}

#[test]
fn immediate_newlines_in_short_unicode_paste_stay_in_draft() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    let now = std::time::Instant::now();

    for (offset, value) in "甲\n乙\n丙".chars().enumerate() {
        let timestamp = now + std::time::Duration::from_millis(offset as u64);
        let action = match value {
            '\n' => UiAction::SubmitCurrentDraft { now: timestamp },
            value => UiAction::InsertPlainChar {
                value,
                now: timestamp,
            },
        };
        let outcome = UiReducer::reduce(&mut state, action);

        assert_eq!(outcome.side_effect, None);
    }

    assert_eq!(state.composer.draft.text(), "甲\n乙\n丙");
    assert!(state.composer.submitted_prompt.is_none());

    let outcome = UiReducer::reduce(
        &mut state,
        UiAction::SubmitCurrentDraft {
            now: now + std::time::Duration::from_millis(20),
        },
    );

    assert_eq!(
        outcome.side_effect,
        Some(crate::ui_workflow::reducer::UiSideEffect::RuntimeSubmit)
    );
    assert_eq!(
        state.composer.submitted_prompt.as_deref(),
        Some("甲\n乙\n丙")
    );
}

#[test]
fn enter_does_not_swallow_slash_command_submission() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("/model".to_owned());

    let outcome = dispatch_key_with_default_driver(
        &mut test_terminal(),
        &mut state,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.composer.draft.text(), "/model ");
}

#[test]
fn enter_accepts_complete_slash_command_for_submission() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("/clear".to_owned());
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    let outcome = dispatch_key(
        &mut terminal,
        &mut state,
        &mut driver,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.composer.draft.text(), "");
}

#[test]
fn enter_in_bottom_selection_view_applies_action_without_submitting() {
    let mut request = launch_request(None);
    request.catalog = AgentTuiCatalog {
        skills: vec!["review".to_owned()],
        ..AgentTuiCatalog::default()
    };
    let mut state = AgentTuiState::new_for_test(&request);
    state.open_skills_menu();
    let mut terminal = test_terminal();
    let mut driver = test_driver();

    let outcome = dispatch_key(
        &mut terminal,
        &mut state,
        &mut driver,
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
    );

    assert_eq!(outcome, TuiEventOutcome::handled(true));
    assert_eq!(state.composer.draft.text(), "$");
    assert!(state.composer_popup_open());
    assert!(state.composer.submitted_prompt.is_none());
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

fn state_with_history() -> AgentTuiState {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.cells.transcript.clear();
    for index in 0..32 {
        state
            .cells
            .transcript
            .push(crate::surface::cell::RenderCellRecord::system(format!(
                "历史行 {index}"
            )));
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

fn append_exec_cell_with_output(state: &mut AgentTuiState, call_id: &str, tool_name: &str) {
    append_exec_cell_with_provider_output(
        state,
        call_id,
        call_id,
        tool_name,
        &format!("输出 {call_id}"),
    );
}

fn append_exec_cell_with_provider_output(
    state: &mut AgentTuiState,
    part_suffix: &str,
    provider_call_id: &str,
    tool_name: &str,
    output: &str,
) {
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_started_event(
            AgentTuiToolExecutionView {
                part_id: format!("tool-part:test:{part_suffix}"),
                request_id: 1,
                provider_call_id: provider_call_id.to_owned(),
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
                part_id: format!("tool-part:test:{part_suffix}"),
                provider_call_id: provider_call_id.to_owned(),
                tool_name: tool_name.to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                stream: "stdout".to_owned(),
                sequence: 1,
                text: output.to_owned(),
            },
        ),
    );
}

fn test_terminal() -> TestAgentTerminal {
    Terminal::new_for_test(CrosstermBackend::new(Vec::new()), 4).expect("测试终端应可创建")
}

fn dispatch_key(
    terminal: &mut TestAgentTerminal,
    state: &mut AgentTuiState,
    driver: &mut RuntimeSubmissionDriver<TestRuntimeAdapter>,
    key: KeyEvent,
) -> TuiEventOutcome {
    dispatch_test_ui_event(terminal, state, driver, UiEvent::Key(key))
        .expect("按键 UiEvent 调度不应失败")
}

fn dispatch_key_with_default_driver(
    terminal: &mut TestAgentTerminal,
    state: &mut AgentTuiState,
    key: KeyEvent,
) -> TuiEventOutcome {
    let mut driver = test_driver();
    dispatch_key(terminal, state, &mut driver, key)
}

#[test]
fn mapped_action_catalog_covers_primary_interaction_paths() {
    let state = state_with_history();
    assert_eq!(
        mapped_action_kind_for_test(
            &state,
            UiEvent::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ),
        None
    );
    assert_eq!(
        mapped_action_kind_for_test(
            &state,
            UiEvent::Key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
        ),
        Some(UiActionKind::PageTranscriptViewUp)
    );
    assert_eq!(
        mapped_action_kind_for_test(
            &state,
            UiEvent::Key(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::SUPER)),
        ),
        Some(UiActionKind::PasteImageFromSystemClipboard)
    );
    assert_ne!(
        mapped_action_kind_for_test(
            &state,
            UiEvent::Key(KeyEvent::new(
                KeyCode::Char('v'),
                KeyModifiers::SUPER | KeyModifiers::SHIFT,
            )),
        ),
        Some(UiActionKind::PasteImageFromSystemClipboard)
    );
}

fn test_driver() -> RuntimeSubmissionDriver<TestRuntimeAdapter> {
    RuntimeSubmissionDriver::new(TestRuntimeAdapter)
}
