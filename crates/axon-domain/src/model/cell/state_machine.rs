//! `RuntimeCell` 生命周期状态机规则。

use super::events::{PausedCellState, RuntimeCellState};
use crate::{AxonError, AxonResult, ToolCallId};

/// `RuntimeCell` 的统一状态机。
pub(super) struct RuntimeCellStateMachine;

impl RuntimeCellStateMachine {
    pub(super) fn start(current: &RuntimeCellState) -> AxonResult<RuntimeCellState> {
        match current {
            RuntimeCellState::Draft | RuntimeCellState::Queued => Ok(RuntimeCellState::Running),
            _ => Err(AxonError::cell_invalid_transition(
                "RuntimeCell 只能从 Draft/Queued 进入 Running",
            )),
        }
    }

    pub(super) fn wait_for_tool(
        current: &RuntimeCellState,
        tool_call_id: ToolCallId,
    ) -> AxonResult<RuntimeCellState> {
        Self::ensure_running(current, "只有 Running cell 能进入 WaitingTool")?;
        Ok(RuntimeCellState::WaitingTool(tool_call_id))
    }

    pub(super) fn complete_tool(
        current: &RuntimeCellState,
    ) -> AxonResult<(RuntimeCellState, ToolCallId)> {
        if let RuntimeCellState::WaitingTool(tool_call_id) = current {
            Ok((RuntimeCellState::Running, *tool_call_id))
        } else {
            Err(AxonError::cell_invalid_transition(
                "只有 WaitingTool cell 能恢复运行",
            ))
        }
    }

    pub(super) fn ensure_tool_call(
        current: &RuntimeCellState,
        tool_call_id: ToolCallId,
        message: &'static str,
    ) -> AxonResult<()> {
        if current == &RuntimeCellState::WaitingTool(tool_call_id) {
            Ok(())
        } else {
            Err(AxonError::cell_invalid_transition(message))
        }
    }

    pub(super) fn pause(
        current: &RuntimeCellState,
    ) -> AxonResult<(RuntimeCellState, PausedCellState)> {
        let previous_state = match current.clone() {
            RuntimeCellState::Running => PausedCellState::Running,
            RuntimeCellState::WaitingTool(tool_call_id) => {
                PausedCellState::WaitingTool(tool_call_id)
            }
            RuntimeCellState::Blocked(reason) => PausedCellState::Blocked(reason),
            RuntimeCellState::Paused(_) => {
                return Err(AxonError::cell_invalid_transition(
                    "RuntimeCell 不能重复暂停",
                ));
            }
            RuntimeCellState::Draft | RuntimeCellState::Queued => {
                return Err(AxonError::cell_invalid_transition(
                    "Draft/Queued RuntimeCell 不能暂停",
                ));
            }
            RuntimeCellState::Completed(_)
            | RuntimeCellState::Failed(_)
            | RuntimeCellState::Cancelled(_) => {
                return Err(AxonError::cell_invalid_transition(
                    "终态 RuntimeCell 不能暂停",
                ));
            }
        };
        Ok((
            RuntimeCellState::Paused(previous_state.clone()),
            previous_state,
        ))
    }

    pub(super) fn resume(
        current: &RuntimeCellState,
    ) -> AxonResult<(RuntimeCellState, RuntimeCellState)> {
        let RuntimeCellState::Paused(previous_state) = current.clone() else {
            return Err(AxonError::cell_invalid_transition(
                "只有 Paused RuntimeCell 能恢复",
            ));
        };
        let resumed_state = previous_state.into_runtime_state();
        Ok((resumed_state.clone(), resumed_state))
    }

    pub(super) fn ensure_not_final(
        current: &RuntimeCellState,
        message: &'static str,
    ) -> AxonResult<()> {
        if matches!(
            current,
            RuntimeCellState::Completed(_)
                | RuntimeCellState::Failed(_)
                | RuntimeCellState::Cancelled(_)
        ) {
            return Err(AxonError::cell_invalid_transition(message));
        }
        Ok(())
    }

    pub(super) fn ensure_running(
        current: &RuntimeCellState,
        message: &'static str,
    ) -> AxonResult<()> {
        if matches!(current, RuntimeCellState::Running) {
            Ok(())
        } else {
            Err(AxonError::cell_invalid_transition(message))
        }
    }
}
