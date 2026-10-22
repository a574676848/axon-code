//! Agent TUI Composer 历史导航回归测试。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Size;

use crate::{r#loop::agent_tui::tui_event::UiEvent, ui_workflow::action_kind::UiActionKind};

use super::{
    event_handlers::mapped_action_kind_for_test, state::AgentTuiState, state_tests::launch_request,
};

#[test]
fn unchanged_explicit_multiline_history_uses_visual_line_boundaries() {
    let mut state = recalled_middle_history_state("第一行\n第二行".to_owned());

    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Up),
        Some(UiActionKind::MoveCursorUp)
    );
    state.move_cursor_up();
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Up),
        Some(UiActionKind::HistoryPrevious)
    );
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Down),
        Some(UiActionKind::MoveCursorDown)
    );
    state.move_cursor_down();
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Down),
        Some(UiActionKind::HistoryNext)
    );

    state.push('!');
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Up),
        Some(UiActionKind::MoveCursorUp)
    );
}

#[test]
fn unchanged_soft_wrapped_history_uses_visual_line_boundaries() {
    let mut state = recalled_middle_history_state("a".repeat(120));
    state.set_frame_size(Size::new(100, 24));

    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Up),
        Some(UiActionKind::MoveCursorUp)
    );
    state.move_cursor_up();
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Up),
        Some(UiActionKind::HistoryPrevious)
    );
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Down),
        Some(UiActionKind::MoveCursorDown)
    );
    state.move_cursor_down();
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Down),
        Some(UiActionKind::HistoryNext)
    );
}

#[test]
fn working_composer_keeps_history_navigation_available() {
    let mut state = recalled_middle_history_state("当前输入".to_owned());
    let submission = state.begin_submit().expect("草稿应可提交");
    state.start_runtime_submit(&submission);

    assert!(state.is_working());
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Up),
        Some(UiActionKind::HistoryPrevious)
    );
    assert_eq!(
        mapped_vertical_action(&state, KeyCode::Down),
        Some(UiActionKind::HistoryNext)
    );
}

#[test]
fn cancel_history_search_restores_structured_draft() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.submit_plain("历史任务".to_owned());
    state.composer.draft.attach_file(
        std::path::PathBuf::from("C:/tmp/search-image.png"),
        super::attachments::AgentTuiAttachmentKind::Image,
    );
    let draft_text = state.composer.draft.text().to_owned();

    state.toggle_history_search();
    state.cancel_history_search();

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

fn recalled_middle_history_state(draft: String) -> AgentTuiState {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.submit_plain("较旧历史".to_owned());
    state.submit_plain(draft);
    state.submit_plain("较新历史".to_owned());
    state.history_previous();
    state.history_previous();
    state
}

fn mapped_vertical_action(state: &AgentTuiState, code: KeyCode) -> Option<UiActionKind> {
    mapped_action_kind_for_test(state, UiEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}
