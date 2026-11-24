//! Agent TUI 提问面板的按键链路测试。
//!
//! 走的是真实链路：crossterm 按键 → `map_key_to_action` → 动作 reducer → 面板视图。
//! 只测视图自身的上下移动是不够的：这个缺陷恰恰出在「按键有没有交到面板手上」，
//! 以及「交到面板之后编辑态里有没有出路」。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use axon_domain::{UserQuestionItem, UserQuestionOption};
use axon_protocol::wire::{WireEventKind, WireUserQuestion};

use crate::{
    r#loop::agent_tui::{
        bottom_view::BottomView, state::AgentTuiState, state_tests::launch_request,
        tui_event::UiEvent, wire_event_test_support::wire_event,
    },
    ui_workflow::{
        action::{UiAction, UiKeyCode, UiKeyEvent},
        action_kind::UiActionKind,
        reducer::{UiActionReducer, UiReducer},
    },
};

use super::event_handlers::mapped_action_kind_for_test;

#[test]
fn question_panel_arrow_keys_reach_the_panel_through_the_real_mapping() {
    let mut state = question_state();

    // 映射层：面板打开时上下键必须交给面板，而不是落到编辑器的历史翻页或光标移动上。
    assert_eq!(
        mapped_action_kind_for_test(&state, UiEvent::Key(question_key(KeyCode::Down))),
        Some(UiActionKind::BottomViewKey),
        "面板打开时下键必须交给面板"
    );
    assert_eq!(panel_focus(&state), 0);

    // 归约层：同一条按键必须真的移动面板焦点。
    UiReducer::reduce(
        &mut state,
        UiAction::BottomViewKey(UiKeyEvent::plain(UiKeyCode::Down)),
    );
    assert_eq!(panel_focus(&state), 1);

    UiReducer::reduce(
        &mut state,
        UiAction::BottomViewKey(UiKeyEvent::plain(UiKeyCode::Up)),
    );
    assert_eq!(panel_focus(&state), 0);
}

#[test]
fn typing_into_the_custom_answer_still_leaves_a_way_back_to_the_options() {
    let mut state = question_state();

    // 键入一个字符即进入自定义答案编辑态。
    UiReducer::reduce(
        &mut state,
        UiAction::BottomViewKey(UiKeyEvent::plain(UiKeyCode::Char('x'))),
    );
    assert!(panel_is_editing(&state), "字符键应进入自定义答案编辑态");
    assert_eq!(panel_custom(&state), "x");

    // 上下键必须退出编辑态并把焦点交回选项列表：这里是此前会把用户困住的地方。
    UiReducer::reduce(
        &mut state,
        UiAction::BottomViewKey(UiKeyEvent::plain(UiKeyCode::Down)),
    );
    assert!(!panel_is_editing(&state), "上下键必须离开自定义答案栏位");
    assert_eq!(panel_focus(&state), 1);
    assert_eq!(panel_custom(&state), "x", "离开编辑态不得丢掉已键入内容");
}

#[test]
fn esc_leaves_editing_without_closing_the_panel() {
    let mut state = question_state();
    UiReducer::reduce(
        &mut state,
        UiAction::BottomViewKey(UiKeyEvent::plain(UiKeyCode::Char('x'))),
    );

    UiReducer::reduce(
        &mut state,
        UiAction::BottomViewKey(UiKeyEvent::plain(UiKeyCode::Esc)),
    );

    assert!(!panel_is_editing(&state));
    assert!(
        state.bottom_view_open(),
        "编辑态 Esc 只退出编辑，不收起面板"
    );
}

fn question_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 一条可作答的提问事件：面板随事件打开。
fn question_state() -> AgentTuiState {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.apply_runtime_event(wire_event(WireEventKind::UserQuestion(WireUserQuestion {
        part_id: "question:keyboard-path".to_owned(),
        call_id: "call-keyboard-path".to_owned(),
        status: "pending".to_owned(),
        questions: vec![sample_question()],
        answers: Vec::new(),
        handoff: None,
        deadline_at_ms: 1_700_000_000_000,
    })));
    state
}

fn sample_question() -> UserQuestionItem {
    UserQuestionItem {
        id: "q1".to_owned(),
        question: "是否继续部署？".to_owned(),
        header: Some("部署确认".to_owned()),
        detail: None,
        options: vec![
            UserQuestionOption {
                label: "继续部署".to_owned(),
                description: None,
            },
            UserQuestionOption {
                label: "暂停观察".to_owned(),
                description: None,
            },
        ],
        multi_select: false,
    }
}

fn question_panel(state: &AgentTuiState) -> &crate::r#loop::agent_tui::bottom_view::QuestionView {
    match state.bottom_view() {
        Some(BottomView::Question(view)) => view,
        _ => panic!("底部面板应为提问面板"),
    }
}

fn panel_focus(state: &AgentTuiState) -> usize {
    question_panel(state).focused_option()
}

fn panel_is_editing(state: &AgentTuiState) -> bool {
    question_panel(state).is_editing()
}

fn panel_custom(state: &AgentTuiState) -> String {
    question_panel(state).custom_input().to_owned()
}
