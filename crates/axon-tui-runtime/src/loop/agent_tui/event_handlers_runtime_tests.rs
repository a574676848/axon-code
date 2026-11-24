//! Agent TUI 运行中 UiEvent 路由测试。

use std::sync::{Arc, Condvar, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::backend::CrosstermBackend;

use crate::{
    r#loop::agent_tui::state::viewport_state::TranscriptViewBoundsCause,
    r#loop::agent_tui::tui_event::UiEvent,
};

use super::{
    AgentTuiCancelToken, AgentTuiRunOutput, AgentTuiRuntimeAdapter, AgentTuiSubmit,
    AgentTuiSubmitFailure, RuntimeEventEmitter,
    custom_terminal::Terminal,
    event_handlers::{TuiEventOutcome, dispatch_runtime_ui_event_for_test, dispatch_test_ui_event},
    run_output::AgentTuiRuntimeEvent,
    state::AgentTuiState,
    state_tests::launch_request,
    submission::RuntimeSubmissionDriver,
};
use crate::surface::cell::{RenderCellData, RenderCellRecord};
use crate::widgets::transcript_view_metrics;

#[test]
fn exit_command_returns_tui_exit_outcome() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("/exit".to_owned());
    let mut terminal =
        Terminal::new_for_test(CrosstermBackend::new(Vec::new()), 4).expect("测试终端应可创建");
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter { gate: None });

    let outcome = dispatch_test_ui_event(
        &mut terminal,
        &mut state,
        &mut driver,
        UiEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    )
    .expect("退出命令调度不应失败");

    assert_eq!(outcome, TuiEventOutcome::Exit);
}

#[test]
fn runtime_paste_routes_through_ui_action_reducer() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("当前任务".to_owned());
    let submission = state.begin_submit().expect("非空提交应启动运行态");
    let gate = BlockingRuntimeGate::new();
    let _gate_guard = gate.guard();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter::blocked(gate.clone()));
    driver.start(submission);

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &mut driver,
        UiEvent::Paste("继续输入".to_owned())
    ));

    assert_eq!(state.composer.draft.text(), "继续输入");
    assert!(state.is_working());
    gate.release();
}

#[test]
fn esc_interrupts_runtime_without_history_dock() {
    let mut state = state_with_history();
    state.replace_draft("需要运行".to_owned());
    let submission = state.begin_submit().expect("非空提交应启动运行态");
    let gate = BlockingRuntimeGate::new();
    let _gate_guard = gate.guard();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter::blocked(gate.clone()));
    driver.start(submission);

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &mut driver,
        UiEvent::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
    ));

    assert!(!state.is_working());
    assert_eq!(state.active_dock_kind(), None);
    gate.release();
}

#[test]
fn session_mutating_commands_are_rejected_while_runtime_continues() {
    for command in ["/clear", "/model", "/think"] {
        let mut state = state_with_history();
        state.replace_draft("需要运行".to_owned());
        let submission = state.begin_submit().expect("非空提交应启动运行态");
        let gate = BlockingRuntimeGate::new();
        let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter::blocked(gate.clone()));
        driver.start(submission);
        let cancel_token = driver.cancel_token().expect("运行态应存在取消令牌");
        state.replace_draft(command.to_owned());

        assert!(dispatch_runtime_ui_event_for_test(
            &mut state,
            &mut driver,
            UiEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        ));

        assert!(!cancel_token.is_cancelled(), "{command} 不应取消当前任务");
        assert!(state.is_working(), "{command} 不应改变运行态");
        assert_eq!(state.composer.draft.text(), command);
        assert!(state.take_queued_submit().is_none(), "{command} 不应排队");

        let Some(RenderCellData::Error(error)) = state
            .cells
            .transcript
            .last()
            .and_then(|line| line.data.as_ref())
        else {
            panic!("{command} 应生成命令拒绝错误");
        };
        assert_eq!(error.title, "命令暂不可用");
        assert!(error.message.contains(command));
        gate.release();
    }
}

#[test]
fn runtime_repeat_key_keeps_composer_editing_responsive() {
    let mut state = AgentTuiState::new_for_test(&launch_request(None));
    state.replace_draft("当前任务".to_owned());
    let submission = state.begin_submit().expect("非空提交应启动运行态");
    let gate = BlockingRuntimeGate::new();
    let _gate_guard = gate.guard();
    let mut driver = RuntimeSubmissionDriver::new(TestRuntimeAdapter::blocked(gate.clone()));
    driver.start(submission);

    assert!(dispatch_runtime_ui_event_for_test(
        &mut state,
        &mut driver,
        UiEvent::Key(KeyEvent::new_with_kind(
            KeyCode::Char('你'),
            KeyModifiers::NONE,
            KeyEventKind::Repeat,
        ))
    ));

    assert_eq!(state.composer.draft.text(), "你");
    assert!(state.is_working());
    gate.release();
}

#[derive(Clone)]
struct BlockingRuntimeGate {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl BlockingRuntimeGate {
    fn new() -> Self {
        Self {
            state: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn wait_until_released(&self) {
        let (lock, cvar) = &*self.state;
        let mut released = lock.lock().expect("gate lock 不应中毒");
        while !*released {
            let result = cvar
                .wait_timeout(released, std::time::Duration::from_secs(5))
                .expect("gate wait 不应中毒");
            released = result.0;
            if result.1.timed_out() {
                break;
            }
        }
    }

    fn release(&self) {
        let (lock, cvar) = &*self.state;
        *lock.lock().expect("gate lock 不应中毒") = true;
        cvar.notify_all();
    }

    fn guard(&self) -> GateGuard {
        GateGuard(self.clone())
    }
}

struct GateGuard(BlockingRuntimeGate);
impl Drop for GateGuard {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[derive(Clone, Default)]
struct TestRuntimeAdapter {
    gate: Option<BlockingRuntimeGate>,
}

impl TestRuntimeAdapter {
    fn blocked(gate: BlockingRuntimeGate) -> Self {
        Self { gate: Some(gate) }
    }
}

impl AgentTuiRuntimeAdapter for TestRuntimeAdapter {
    fn submit(
        &self,
        _submission: &AgentTuiSubmit,
        _cancel_token: &AgentTuiCancelToken,
        _emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure> {
        if let Some(gate) = &self.gate {
            gate.wait_until_released();
        }
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
