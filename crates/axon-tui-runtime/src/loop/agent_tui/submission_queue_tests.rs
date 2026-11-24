use axon_protocol::wire::{WireEventKind, WireUserMessage};

use super::test_support::*;
use super::*;
use crate::r#loop::agent_tui::run_output::{AgentTuiToolCompletionView, AgentTuiToolKind};

#[test]
fn restored_edit_requeues_and_starts_after_current_turn_finishes() {
    let mut state = test_state();
    let first = test_submission("当前轮");
    state.start_runtime_submit(&first);
    state.replace_draft("下一轮".to_owned());
    assert!(state.queue_draft_for_next_turn());
    assert!(state.restore_interrupt_input_to_draft());
    state.insert_str("（已编辑）");
    assert!(state.queue_draft_for_next_turn());

    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(first);
    gate.release();
    wait_for_driver_dirty_tick(&mut driver, &mut state);

    assert!(driver.is_active());
    assert_eq!(
        state.composer.submitted_prompt.as_deref(),
        Some("下一轮（已编辑）")
    );
    assert!(state.take_queued_submit().is_none());
}

#[test]
fn tool_completion_keeps_current_turn_and_fifo_starts_after_finish() {
    let mut state = test_state();
    let first = test_submission("第一轮");
    state.start_runtime_submit(&first);
    state.replace_draft("第二轮".to_owned());
    assert!(state.queue_draft_for_next_turn());
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(first);

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::tool_completed_event(
            AgentTuiToolCompletionView {
                part_id: "tool-part:test:auto-queue".to_owned(),
                provider_call_id: "call-auto-queue".to_owned(),
                tool_name: "shell_command".to_owned(),
                kind: "shell".to_owned(),
                semantic_kind: AgentTuiToolKind::Shell,
                status: "completed".to_owned(),
                summary: "工具已完成".to_owned(),
                metadata: None,
                error_type: None,
                filesystem_write_performed: false,
                duration_ms: Some(10),
            },
        ),
    );

    assert!(!state.interrupt_requested());
    assert!(driver.is_active());
    assert_eq!(state.composer.submitted_prompt.as_deref(), Some("第一轮"));

    gate.release();
    wait_for_driver_dirty_tick(&mut driver, &mut state);

    assert!(driver.is_active());
    assert_eq!(state.composer.submitted_prompt.as_deref(), Some("第二轮"));
    assert!(!state.interrupt_requested());
}

#[test]
fn safe_boundary_user_message_commits_pending_steer_to_transcript() {
    let mut state = test_state();
    state.replace_draft("补充当前 turn".to_owned());
    assert!(state.queue_draft_for_next_turn());
    let (submission, history_record) = state
        .take_steer_submission_from_queue()
        .expect("普通消息应可进入 steer inbox");
    state.record_pending_steer(submission, history_record);

    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("等待注入 1. 补充当前 turn")
    );
    assert!(!state.cells.transcript.iter().any(|line| {
        line.role == crate::surface::cell::ROLE_USER && line.text == "补充当前 turn"
    }));

    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::wire_event(WireEventKind::UserMessage(
            WireUserMessage {
                part_id: "steer-user-message:1".to_owned(),
                message_id: "steer-user-message:1".to_owned(),
                content: "补充当前 turn".to_owned(),
                local_files: Vec::new(),
            },
        )),
    );

    assert!(state.queued_submit_preview().is_none());
    assert!(state.cells.transcript.iter().any(|line| {
        line.role == crate::surface::cell::ROLE_USER && line.text == "补充当前 turn"
    }));
}
