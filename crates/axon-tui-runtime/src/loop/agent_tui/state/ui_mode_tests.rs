use crate::app_shell::composer_dock::DockKind;
use crate::r#loop::agent_tui::{
    AgentTuiRunOutput, state::AgentTuiState, state_tests::launch_request,
};

use super::ui_mode::{UiActivity, UiMode};

#[test]
fn ui_mode_prioritizes_dock_over_composer_popup_and_working() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));

    state.start_runtime_submit(&super::runtime_flow_test_support::test_submission(
        "等待回复",
    ));
    state.insert_str("$review");
    state.toggle_shortcut_help_dock();

    assert_eq!(state.ui_mode(), UiMode::Dock(DockKind::ShortcutHelp));
    assert_eq!(state.ui_mode_snapshot().activity(), UiActivity::Working);
}

#[test]
fn ui_activity_stays_working_when_dock_is_open() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));

    state.start_runtime_submit(&super::runtime_flow_test_support::test_submission(
        "等待回复",
    ));
    state.toggle_shortcut_help_dock();

    let snapshot = state.ui_mode_snapshot();

    assert_eq!(snapshot.mode, UiMode::Dock(DockKind::ShortcutHelp));
    assert_eq!(snapshot.activity(), UiActivity::Working);
}

#[test]
fn ui_activity_returns_to_idle_after_failed_submit() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));

    state.start_runtime_submit(&super::runtime_flow_test_support::test_submission(
        "失败后继续输入",
    ));
    state.finish_submit(Err(crate::RuntimeError::runtime("provider failed")));

    let snapshot = state.ui_mode_snapshot();

    assert_eq!(snapshot.activity(), UiActivity::Idle);
}

#[test]
fn ui_activity_reports_queued_as_single_render_state() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));

    state.start_runtime_submit(&super::runtime_flow_test_support::test_submission(
        "等待回复",
    ));
    state.insert_str("下一轮");
    state.queue_draft_for_next_turn();
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));

    let snapshot = state.ui_mode_snapshot();

    assert_eq!(snapshot.activity(), UiActivity::Queued);
}

#[test]
fn terminal_title_context_uses_workspace_name_before_session_title() {
    let mut request = launch_request(None);
    request.cwd = std::path::PathBuf::from("zbg").join("axon-code");
    let state = AgentTuiState::new_for_test(&request);

    let context = state.terminal_title_context();

    assert_eq!(context.workspace_name, Some("axon-code"));
}
