use std::{
    sync::{Arc, Condvar, Mutex, mpsc},
    time::{Duration, Instant},
};

use super::*;
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiLaunchRequest, AgentTuiRunOverrides,
    AgentTuiRuntimeAdapter, AgentTuiSteerResult, AgentTuiSubmitFailure, RuntimeError,
    RuntimeEventEmitter,
    input::AgentTuiInputResult,
    run_output::{AgentTuiRunOutput, AgentTuiRuntimeEvent},
};
use crate::surface::runtime_adapter::{
    RuntimeAdapterCapabilities, RuntimeAdapterContract, RuntimeAdapterSchemaVersion,
};
use crate::surface::{
    cell::{RenderCellData, RenderCellRecord, STATUS_KIND_INTERRUPT_REQUESTED},
    cell_store::RenderCellList,
};

pub(super) fn test_launch_request() -> AgentTuiLaunchRequest {
    AgentTuiLaunchRequest {
        cwd: axon_test_support::test_temp_root(),
        prompt: None,
        workspace_instruction_paths: Vec::new(),
        config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
        overrides: AgentTuiRunOverrides::default(),
        catalog: AgentTuiCatalog::default(),
        runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
    }
}

pub(super) fn test_state() -> AgentTuiState {
    AgentTuiState::new_for_test(&test_launch_request())
}

pub(super) fn test_driver() -> RuntimeSubmissionDriver<TestRuntimeAdapter> {
    RuntimeSubmissionDriver::new(TestRuntimeAdapter::default())
}

pub(super) fn wait_for_driver_dirty_tick(
    driver: &mut RuntimeSubmissionDriver<TestRuntimeAdapter>,
    state: &mut AgentTuiState,
) {
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if driver.tick(state).expect("driver tick 不应失败") {
            return;
        }
        std::thread::yield_now();
    }
    panic!("driver 未在 500 毫秒内处理 worker 消息");
}

#[derive(Clone, Default)]
pub(super) struct TestRuntimeAdapter {
    pub(super) steer_result: AgentTuiSteerResult,
    pub(super) capabilities: RuntimeAdapterCapabilities,
    pub(super) steer_tx: Option<mpsc::Sender<String>>,
    pub(super) finish_tx: Option<mpsc::Sender<()>>,
    pub(super) panic_on_submit: bool,
    pub(super) progress_event_count: usize,
    pub(super) submit_gate: Option<TestRuntimeGate>,
    /// Daemon 侧可恢复的活动回合；`None` 表示当前工作区没有正在执行的主 turn。
    pub(super) active_turn: Option<crate::r#loop::agent_tui::AgentTuiActiveTurn>,
}

impl AgentTuiRuntimeAdapter for TestRuntimeAdapter {
    fn contract(&self) -> RuntimeAdapterContract {
        RuntimeAdapterContract::new(RuntimeAdapterSchemaVersion::new(1, 0), self.capabilities)
    }

    fn load_active_turn(
        &self,
        _request: &AgentTuiLaunchRequest,
    ) -> Option<crate::r#loop::agent_tui::AgentTuiActiveTurn> {
        self.active_turn.clone()
    }

    fn submit(
        &self,
        _submission: &AgentTuiSubmit,
        cancel_token: &AgentTuiCancelToken,
        emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiRunOutput, AgentTuiSubmitFailure> {
        assert!(!self.panic_on_submit, "test runtime panic");
        if let Some(gate) = &self.submit_gate {
            gate.wait(cancel_token);
        }
        for index in 0..self.progress_event_count {
            emit.emit(
                crate::r#loop::agent_tui::wire_event_test_support::assistant_delta_event(format!(
                    "{index},"
                )),
            );
        }
        if let Some(tx) = &self.finish_tx {
            let _ = tx.send(());
        }
        Ok(AgentTuiRunOutput::assistant("完成"))
    }

    fn steer(
        &self,
        submission: &AgentTuiSubmit,
        _emit: &mut RuntimeEventEmitter<'_, AgentTuiRuntimeEvent>,
    ) -> Result<AgentTuiSteerResult, RuntimeError> {
        if let Some(tx) = &self.steer_tx {
            let _ = tx.send(submission.text().to_owned());
        }
        Ok(self.steer_result)
    }
}

#[derive(Clone, Default)]
pub(super) struct TestRuntimeGate {
    state: Arc<(Mutex<bool>, Condvar)>,
}

impl TestRuntimeGate {
    pub(super) fn wait(&self, cancel_token: &AgentTuiCancelToken) {
        let (lock, cvar) = &*self.state;
        let mut released = lock.lock().expect("test runtime gate mutex poisoned");
        while !*released && !cancel_token.is_cancelled() {
            let result = cvar
                .wait_timeout(released, std::time::Duration::from_millis(5))
                .expect("test runtime gate condvar poisoned");
            released = result.0;
        }
    }

    pub(super) fn release(&self) {
        let (lock, cvar) = &*self.state;
        *lock.lock().expect("test runtime gate mutex poisoned") = true;
        cvar.notify_all();
    }
}

pub(super) fn test_submission(text: &str) -> AgentTuiSubmit {
    AgentTuiSubmit {
        input: AgentTuiInputResult::Submit {
            text: text.to_owned(),
            elements: Vec::new(),
        },
        overrides: AgentTuiRunOverrides::default(),
        cwd: axon_test_support::test_temp_root(),
        question_answer: None,
    }
}

pub(super) fn typed_error_details(state: &AgentTuiState) -> Vec<String> {
    state
        .cells
        .transcript
        .iter()
        .find_map(|line| match &line.data {
            Some(RenderCellData::Error(cell)) => Some(cell.details.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

pub(super) fn has_interrupt_status_cell(lines: &RenderCellList) -> bool {
    lines.iter().any(is_interrupt_status_cell)
}

pub(super) fn is_interrupt_status_cell(line: &RenderCellRecord) -> bool {
    let Some(RenderCellData::Status(cell)) = &line.data else {
        return false;
    };
    cell.title == "中断"
        && cell.items.iter().any(|item| {
            item.kind == STATUS_KIND_INTERRUPT_REQUESTED && item.summary == "已停止，可继续输入"
        })
}
