use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use axon_agent_runtime::ProjectionRuntimeBudget;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use super::test_support::*;
use super::*;
use crate::r#loop::agent_tui::{
    AgentTuiRunOutput, AgentTuiSteerResult, AgentTuiSubmitFailure, RuntimeError,
};
use crate::surface::cell::{RenderCellData, RenderCellRecord};
use crate::surface::cell_event_data::AgentRenderExecCompletion;
use crate::surface::error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity};
use crate::surface::runtime_adapter::RuntimeAdapterCapabilities;

#[test]
fn esc_while_runtime_working_cancels_current_task_without_exiting_tui() {
    let mut state = test_state();
    let submission = test_submission("生成实现");
    state.start_runtime_submit(&submission);
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");

    assert!(state.is_working());
    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));

    assert!(cancel_token.is_cancelled());
    assert!(!state.should_exit());
    assert!(!state.is_working());
    assert!(has_interrupt_status_cell(&state.cells.transcript));
    let status_count = state
        .cells
        .transcript
        .iter()
        .filter(|line| is_interrupt_status_cell(line))
        .count();
    state.request_interrupt_current_task();
    assert_eq!(
        state
            .cells
            .transcript
            .iter()
            .filter(|line| is_interrupt_status_cell(line))
            .count(),
        status_count
    );
    gate.release();
}

#[test]
fn esc_with_unsubmitted_draft_interrupts_and_preserves_composer_draft() {
    let mut state = test_state();
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    state.replace_draft("立即执行下一轮".to_owned());
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));

    assert!(cancel_token.is_cancelled());
    assert!(!state.is_working());
    assert_eq!(state.composer.draft.text(), "立即执行下一轮");
    assert!(state.take_queued_submit().is_none());
    assert!(state.queued_submit_preview().is_none());
    gate.release();
}

#[test]
fn alt_e_after_interrupt_without_output_restores_cancelled_prompt() {
    let mut state = test_state();
    let submission = test_submission("尚未输出的任务");
    state.start_runtime_submit(&submission);
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));

    assert!(state.restore_interrupt_input_to_draft());
    assert_eq!(state.composer.draft.text(), "尚未输出的任务");
    assert!(state.take_queued_submit().is_none());
    gate.release();
}

#[test]
fn alt_e_after_visible_output_restores_only_follow_up_inputs() {
    let mut state = test_state();
    let submission = test_submission("已有输出的任务");
    state.start_runtime_submit(&submission);
    state.apply_runtime_event(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "已经开始回答".to_owned(),
        ),
    );
    state.replace_draft("后续补充".to_owned());
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));

    assert!(state.restore_interrupt_input_to_draft());
    assert_eq!(state.composer.draft.text(), "后续补充");
    assert!(!state.composer.draft.text().contains("已有输出的任务"));
    gate.release();
}

#[test]
fn runtime_typing_keeps_composer_editable_without_waiting_for_finish() {
    let mut state = test_state();
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    let cancel_token = AgentTuiCancelToken::default();
    let mut driver = test_driver();

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Char('你'), KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));

    assert_eq!(state.composer.draft.text(), "你");
    assert!(state.is_working());
}

#[test]
fn runtime_enter_sends_steer_and_escape_restarts_pending_message_as_next_turn() {
    let mut state = test_state();
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    state.replace_draft("补充当前 turn".to_owned());
    let (steer_tx, steer_rx) = mpsc::channel();
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        steer_result: AgentTuiSteerResult::Accepted,
        capabilities: RuntimeAdapterCapabilities {
            steer: true,
            ..RuntimeAdapterCapabilities::AXON_CLI
        },
        steer_tx: Some(steer_tx),
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));
    wait_for_driver_dirty_tick(&mut driver, &mut state);

    assert_eq!(
        steer_rx
            .try_recv()
            .expect("Enter 应把队首输入交给 runtime steer inbox"),
        "补充当前 turn"
    );
    assert!(state.has_queued_submit());
    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("等待注入 1. 补充当前 turn")
    );
    assert!(!state.cells.transcript.iter().any(|line| {
        line.role == crate::surface::cell::ROLE_USER && line.text == "补充当前 turn"
    }));
    state.replace_draft("排在后面的消息".to_owned());
    assert!(state.queue_draft_for_next_turn());
    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));

    assert!(cancel_token.is_cancelled());
    assert!(state.interrupt_requested());
    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("待发送 1. 补充当前 turn\n待发送 2. 排在后面的消息")
    );

    assert!(driver.tick(&mut state).expect("中断 tick 不应失败"));
    assert!(driver.is_active());
    assert!(!state.interrupt_requested());
    assert_eq!(
        state.composer.submitted_prompt.as_deref(),
        Some("补充当前 turn")
    );
    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("待发送 1. 排在后面的消息")
    );
    gate.release();
}

#[test]
fn rejected_escape_steer_returns_message_to_fifo_without_interrupting() {
    let mut state = test_state();
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    state.replace_draft("稍后继续".to_owned());
    let gate = TestRuntimeGate::default();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        steer_result: AgentTuiSteerResult::Rejected,
        capabilities: RuntimeAdapterCapabilities {
            steer: true,
            ..RuntimeAdapterCapabilities::AXON_CLI
        },
        submit_gate: Some(gate.clone()),
        ..TestRuntimeAdapter::default()
    });
    driver.start(submission);
    let cancel_token = driver
        .cancel_token()
        .expect("active driver 应暴露 cancel token");

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));
    wait_for_driver_dirty_tick(&mut driver, &mut state);
    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("待发送 1. 稍后继续")
    );

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));
    wait_for_driver_dirty_tick(&mut driver, &mut state);

    assert_eq!(
        state.queued_submit_preview().as_deref(),
        Some("待发送 1. 稍后继续")
    );
    assert!(state.cells.transcript.iter().any(|line| {
        matches!(
            &line.data,
            Some(RenderCellData::Status(cell))
                if cell.title == "Steer"
                    && cell.items.iter().any(|item| item.summary.contains("拒绝 steer"))
        )
    }));
    assert!(driver.is_active());
    assert!(!cancel_token.is_cancelled());
    assert!(!state.interrupt_requested());
    gate.release();
}

#[test]
fn runtime_alt_up_does_not_select_primary_history_cell() {
    let mut state = test_state();
    state
        .cells
        .transcript
        .push(RenderCellRecord::exec_with_data(
            crate::surface::cell_event_data::AgentRenderExecCell::completed_with_id(
                AgentRenderExecCompletion {
                    id: "exec-part:tool-1".to_owned(),
                    tool_name: "builtin.shell_command".to_owned(),
                    status: "completed".to_owned(),
                    provider_call_id: "tool-1".to_owned(),
                    summary: Some("cargo check 完成".to_owned()),
                    arguments_preview: None,
                    output_preview: None,
                    duration_ms: None,
                },
            ),
            crate::surface::cell::RenderCellAction::Ran,
        ));
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    let cancel_token = AgentTuiCancelToken::default();
    let mut driver = test_driver();

    assert!(!dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
        &cancel_token,
        &mut driver,
    ));
    assert_eq!(state.viewport.selected_runtime_render_index(), None);
}

#[test]
fn worker_progress_drain_is_budgeted_to_keep_runtime_input_fair() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("长任务持续输出进度"));
    let (tx, rx) = mpsc::channel();
    for index in 0..=WORKER_MESSAGE_DRAIN_BUDGET {
        tx.send(SubmissionWorkerMessage::Progress(Box::new(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(format!(
                "{index},"
            )),
        )))
        .expect("测试进度事件应写入成功");
    }
    let mut notifications = TuiNotificationManager::default();

    let (finished, dirty) =
        drain_worker_messages(&mut state, &rx, &mut notifications).expect("drain 应成功");

    assert!(finished.is_none());
    assert!(dirty);
    assert!(rx.try_recv().is_ok());
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));
    let committed = state
        .cells
        .transcript
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(committed.contains("0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,"));
    assert!(!committed.contains("16,"));
}

#[test]
fn assistant_delta_batch_flushes_at_limit_without_waiting_for_finish() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("长任务持续输出"));
    let (tx, rx) = mpsc::channel();
    let delta_chunk_count = ProjectionRuntimeBudget::current().delta_chunk_count;
    for index in 0..=delta_chunk_count {
        tx.send(SubmissionWorkerMessage::Progress(Box::new(
            crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(format!(
                "{index},"
            )),
        )))
        .expect("测试增量事件应写入成功");
    }
    let mut notifications = TuiNotificationManager::default();
    let mut last_runtime_event_id = None;
    let mut pending = PendingAssistantDeltaBatch::default();
    let mut pending_started_at = None;

    let drain_rounds = (delta_chunk_count + WORKER_MESSAGE_DRAIN_BUDGET)
        .saturating_div(WORKER_MESSAGE_DRAIN_BUDGET);
    let mut observed_dirty = false;
    for _ in 0..drain_rounds {
        let (finished, dirty, _) = drain_worker_messages_with_event_id(
            &mut state,
            &rx,
            &mut notifications,
            &mut last_runtime_event_id,
            &mut pending,
            &mut pending_started_at,
            false,
        )
        .expect("drain 应成功");
        assert!(finished.is_none());
        observed_dirty |= dirty;
    }

    assert!(observed_dirty);
    assert!(pending.is_empty());
    state.finish_submit(Ok(AgentTuiRunOutput::empty()));
    let committed = state
        .cells
        .transcript
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(committed.contains("0,1,2,3,4,"));
    assert!(committed.contains(&format!("{delta_chunk_count},")));
}

#[test]
fn assistant_delta_batch_policy_flushes_newline_and_merge_window() {
    let start = Instant::now();
    let pending = vec!["partial".to_owned()];
    assert!(!assistant_delta_batch_should_flush(
        &pending,
        Some(start),
        start + ASSISTANT_DELTA_MERGE_WINDOW / 2
    ));
    assert!(assistant_delta_batch_should_flush(
        &pending,
        Some(start),
        start + ASSISTANT_DELTA_MERGE_WINDOW
    ));
    assert!(assistant_delta_batch_should_flush(
        &["semantic\n".to_owned()],
        Some(start),
        start
    ));
}

#[test]
fn first_assistant_delta_flushes_without_waiting_for_merge_window() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("流式窗口"));
    let (tx, rx) = mpsc::channel();
    tx.send(SubmissionWorkerMessage::Progress(Box::new(
        crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(
            "partial".to_owned(),
        ),
    )))
    .expect("测试增量事件应写入成功");
    let mut notifications = TuiNotificationManager::default();
    let mut last_runtime_event_id = None;
    let mut pending = PendingAssistantDeltaBatch::default();
    let mut pending_started_at = None;

    let (finished, dirty, _) = drain_worker_messages_with_event_id(
        &mut state,
        &rx,
        &mut notifications,
        &mut last_runtime_event_id,
        &mut pending,
        &mut pending_started_at,
        false,
    )
    .expect("drain 应成功");

    assert!(finished.is_none());
    assert!(dirty);
    assert!(pending.is_empty());
    assert!(pending_started_at.is_none());
}

#[test]
fn worker_failure_error_cell_traces_last_runtime_event_id() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("长任务失败"));
    let (tx, rx) = mpsc::channel();
    let event = crate::r#loop::agent_tui::wire_event_test_support::lifecycle_status_event(
        crate::r#loop::agent_tui::run_output::AgentTuiLifecycleStatusView::new(
            "生成",
            "Generation",
            "生成回复",
            "generation",
            None,
            None,
            None,
        ),
    );
    let expected = format!("事件 runtime-event-{}", event.event_id.get());
    tx.send(SubmissionWorkerMessage::Progress(Box::new(event)))
        .expect("进度事件应写入成功");
    tx.send(SubmissionWorkerMessage::Finished(Err(
        AgentTuiSubmitFailure::without_output(RuntimeError::runtime("provider failed")),
    )))
    .expect("失败事件应写入成功");
    let mut notifications = TuiNotificationManager::default();
    let mut last_runtime_event_id = None;
    let mut pending = PendingAssistantDeltaBatch::default();
    let mut pending_started_at = None;

    let (finished, _, _) = drain_worker_messages_with_event_id(
        &mut state,
        &rx,
        &mut notifications,
        &mut last_runtime_event_id,
        &mut pending,
        &mut pending_started_at,
        true,
    )
    .expect("drain 应成功");
    state.finish_submit_with_cause(finished.expect("应收到失败结果"), last_runtime_event_id);

    assert!(
        typed_error_details(&state)
            .iter()
            .any(|detail| detail == &expected)
    );
}

#[test]
fn worker_panic_projects_fatal_runtime_error_cell_and_recovers_driver() {
    let mut state = test_state();
    let submission = test_submission("触发 panic");
    state.start_runtime_submit(&submission);
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        panic_on_submit: true,
        ..TestRuntimeAdapter::default()
    });

    driver.start(submission);
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if driver.tick(&mut state).expect("panic 应转成 error cell") {
            break;
        }
        std::thread::yield_now();
    }

    assert!(!driver.is_active());
    let error = state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Error(cell)) => Some(cell),
            _ => None,
        })
        .expect("worker panic 应投影为 error cell");
    assert_eq!(error.severity, TuiErrorSeverity::Fatal);
    assert_eq!(error.action, TuiErrorAction::NewSession);
    assert_eq!(error.cell_kind, TuiErrorCellKind::Runtime);
    assert!(
        error
            .details
            .iter()
            .any(|detail| detail == "代码 runtime_worker_panic")
    );
}

#[test]
fn runtime_driver_finishes_turn_and_starts_next_queued_submission() {
    let mut state = test_state();
    let first = test_submission("第一轮");
    state.start_runtime_submit(&first);
    state.replace_draft("第二轮".to_owned());
    assert!(state.queue_draft_for_next_turn());
    let (finished_tx, finished_rx) = mpsc::channel();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
        finish_tx: Some(finished_tx),
        ..TestRuntimeAdapter::default()
    });

    driver.start(first);
    finished_rx.recv().expect("worker 应完成第一轮");

    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        let _ = driver.tick(&mut state).expect("driver tick 不应失败");
        if state.composer.submitted_prompt.as_deref() == Some("第二轮") {
            break;
        }
        std::thread::yield_now();
    }

    assert!(driver.is_active());
    assert!(state.is_working());
    assert_eq!(state.composer.submitted_prompt.as_deref(), Some("第二轮"));
}

#[test]
fn dropping_runtime_driver_reaps_worker_after_cancellation() {
    let gate = TestRuntimeGate::default();
    let (finished_tx, finished_rx) = mpsc::channel();
    let submission = test_submission("回收后台任务");

    {
        let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter {
            finish_tx: Some(finished_tx),
            submit_gate: Some(gate.clone()),
            ..TestRuntimeAdapter::default()
        });
        driver.start(submission);
        gate.release();
    }

    finished_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("driver 析构前应回收 worker");
}

#[test]
fn runtime_enter_preserves_paste_burst_newline_semantics() {
    let mut state = test_state();
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    let cancel_token = AgentTuiCancelToken::default();
    let mut driver = test_driver();
    let now = Instant::now();

    for (offset, value) in ['a', 'b', 'c'].into_iter().enumerate() {
        state.handle_plain_char(value, now + Duration::from_millis(offset as u64));
    }

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));
    assert!(state.flush_paste_burst_if_due(Instant::now() + Duration::from_millis(200)));

    assert!(state.composer.draft.text().ends_with('\n'));
    assert!(state.take_queued_submit().is_none());
    assert!(state.is_working());
}

#[test]
fn runtime_enter_preserves_unicode_paste_burst_newline_semantics() {
    let mut state = test_state();
    let submission = test_submission("当前长任务");
    state.start_runtime_submit(&submission);
    let cancel_token = AgentTuiCancelToken::default();
    let mut driver = test_driver();
    let now = Instant::now();

    for (offset, value) in ['多', '行', '粘', '贴'].into_iter().enumerate() {
        state.handle_plain_char(value, now + Duration::from_millis(offset as u64));
    }

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));
    assert_eq!(state.composer.draft.text(), "多行粘贴\n");
    assert!(state.take_queued_submit().is_none());

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        &cancel_token,
        &mut driver,
    ));
    assert!(state.take_queued_submit().is_some());
    assert!(state.is_working());
}
