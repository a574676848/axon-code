//! 用户问答实时投影与作答提交测试（UQ-205）。

use axon_domain::{UserAnswerItem, UserQuestionItem, UserQuestionOption};
use axon_protocol::wire::{WireInputMode, WireInputPanelState, WireUsageUpdate};
use axon_protocol::wire::{
    WireRenderTimelineItem, WireRenderTimelineItemKind, WireResumeSnapshot, WireSchemaVersion,
    WireSessionRef, WireStatusBarState, WireUserMessage, WireUserQuestion, WireViewportSnapshot,
};

use super::runtime_flow_test_support::*;
use crate::r#loop::agent_tui::{
    bottom_view::BottomView, run_output::AgentTuiRuntimePayload,
    wire_event_test_support::wire_event,
};
use crate::surface::cell::{RenderCellData, RenderCellKind};
use crate::ui_workflow::action::{UiKeyCode, UiKeyEvent};

#[test]
fn live_user_question_event_lands_cell_and_opens_panel() {
    let mut state = test_state();

    state.apply_runtime_event(wire_event(
        axon_protocol::wire::WireEventKind::UserQuestion(pending_question("pending")),
    ));

    assert_eq!(cell_count(&state, RenderCellKind::QUESTION), 1);
    let question = question_data(&state);
    assert_eq!(question.call_id, "call-live-question");
    assert_eq!(question.status, "pending");
    assert_eq!(question.questions.len(), 1);
    assert!(state.bottom_view_open(), "可作答的提问必须弹出底部面板");
    let Some(BottomView::Question(view)) = state.bottom_view() else {
        panic!("底部面板应为提问面板");
    };
    assert_eq!(view.call_id(), "call-live-question");
    assert_eq!(view.lane_id(), "test-session");
}

#[test]
fn extended_user_question_keeps_panel_open_while_settled_closes_it() {
    let mut state = test_state();

    state.apply_runtime_event(wire_event(
        axon_protocol::wire::WireEventKind::UserQuestion(pending_question("extended")),
    ));
    assert!(state.bottom_view_open());

    state.apply_runtime_event(wire_event(
        axon_protocol::wire::WireEventKind::UserQuestion(settled_question()),
    ));

    // 状态流转必须原位刷新同一条单元，而不是再堆一条。
    assert_eq!(cell_count(&state, RenderCellKind::QUESTION), 1);
    let question = question_data(&state);
    assert_eq!(question.status, "settled");
    assert_eq!(question.answers.len(), 1);
    assert_eq!(question.answers[0].selected, vec!["继续部署".to_owned()]);
    assert!(!state.bottom_view_open(), "已结算的提问必须收起面板");
}

#[test]
fn panel_submit_goes_through_question_answer_payload_not_plain_text() {
    let mut state = test_state();
    state.apply_runtime_event(wire_event(
        axon_protocol::wire::WireEventKind::UserQuestion(pending_question("pending")),
    ));

    // 单选默认聚焦首项，Enter 提交该题。
    let submission = state
        .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Enter))
        .expect("作答应产出提交载荷");

    assert!(
        submission.is_question_answer(),
        "作答必须带结构化问答载荷，不能退化成普通消息提交"
    );
    assert_eq!(submission.text(), "", "作答不得自造可见正文");
    let answer = submission
        .question_answer
        .expect("问答载荷应随提交一起交出");
    assert_eq!(answer.lane_id, "test-session");
    assert_eq!(answer.call_id, "call-live-question");
    assert_eq!(answer.answers.len(), 1);
    assert_eq!(answer.answers[0].id, "q1");
    assert_eq!(answer.answers[0].selected, vec!["继续部署".to_owned()]);
}

#[test]
fn skip_encodes_empty_selection_and_keeps_panel_until_delivery() {
    let mut state = test_state();
    state.apply_runtime_event(wire_event(
        axon_protocol::wire::WireEventKind::UserQuestion(pending_question("pending")),
    ));

    let submission = state
        .handle_bottom_view_key(UiKeyEvent::plain(UiKeyCode::Tab))
        .expect("显式跳过应产出提交载荷");
    let answer = submission.question_answer.expect("跳过同样走答案入口");

    assert!(answer.answers[0].selected.is_empty());
    assert_eq!(answer.answers[0].custom, None);
    assert!(
        state.bottom_view_open(),
        "作答未送达前面板必须保持打开以便重试"
    );
}

#[test]
fn visible_user_message_drops_receipt_block_with_encoder_parity() {
    // 与 `axon-cli` 的唯一编码器 `user_question::content` 逐字同形的正文。
    let encoded = "1. 是否继续部署？ 回答：确认部署\n\n[问答回执]\n回答待决问题：call-deploy-99\n回答内容：[{\"id\":\"q1\",\"selected\":[\"确认部署\"]}]";
    let mut state = test_state();

    state.apply_runtime_event(runtime_event(AgentTuiRuntimePayload::WireResumeSnapshot(
        Box::new(resume_snapshot_with_user_message(encoded)),
    )));

    let texts = state
        .cells
        .transcript
        .iter()
        .filter(|line| line.role == crate::surface::cell::ROLE_USER)
        .map(|line| line.text.clone())
        .collect::<Vec<_>>();
    assert_eq!(texts, vec!["1. 是否继续部署？ 回答：确认部署".to_owned()]);
    assert!(
        texts.iter().all(|text| !text.contains("[问答回执]")),
        "可见正文不得残留回执块标题"
    );
    assert!(
        texts.iter().all(|text| !text.contains("回答内容：")),
        "可见正文不得残留答案 JSON 引导行"
    );
}

fn question_data(
    state: &crate::r#loop::agent_tui::state::AgentTuiState,
) -> crate::surface::cell_question_data::AgentRenderQuestionCell {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Question(cell)) => Some(cell.clone()),
            _ => None,
        })
        .expect("转录应包含提问单元")
}

fn pending_question(status: &str) -> WireUserQuestion {
    WireUserQuestion {
        part_id: "question:live".to_owned(),
        call_id: "call-live-question".to_owned(),
        status: status.to_owned(),
        questions: vec![sample_question()],
        answers: Vec::new(),
        handoff: None,
        deadline_at_ms: 1_700_000_000_000,
    }
}

fn settled_question() -> WireUserQuestion {
    WireUserQuestion {
        answers: vec![UserAnswerItem {
            id: "q1".to_owned(),
            selected: vec!["继续部署".to_owned()],
            custom: None,
        }],
        ..pending_question("settled")
    }
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

fn resume_snapshot_with_user_message(content: &str) -> WireResumeSnapshot {
    WireResumeSnapshot {
        schema_version: WireSchemaVersion::default(),
        session: WireSessionRef {
            session_id: "session-question".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        status_bar: WireStatusBarState {
            title: "Axon".to_owned(),
            subtitle: None,
            lifecycle_status: axon_protocol::wire::WireLifecycleStatus::Completed,
            cwd: None,
            provider: None,
            model: None,
            selected_skill_name: None,
            usage: Some(WireUsageUpdate {
                input_tokens: 0,
                output_tokens: 0,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                reasoning_output_tokens: 0,
                current_context_tokens: 0,
                system_prompt_tokens: 0,
                tool_definition_tokens: 0,
                conversation_tokens: 0,
            }),
        },
        viewport: WireViewportSnapshot::default(),
        input_panel: WireInputPanelState {
            mode: WireInputMode::Prompt,
            draft: None,
        },
        timeline: vec![WireRenderTimelineItem {
            part_id: "user:question-receipt".to_owned(),
            sequence: 0,
            recorded_at_ms: 1,
            kind: WireRenderTimelineItemKind::UserMessage(WireUserMessage {
                part_id: "user:question-receipt".to_owned(),
                message_id: "user:question-receipt".to_owned(),
                content: content.to_owned(),
                local_files: Vec::new(),
            }),
        }],
    }
}
