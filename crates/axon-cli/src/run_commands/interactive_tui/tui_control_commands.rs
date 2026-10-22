//! Agent TUI 通用控制命令执行器。

use std::path::Path;

use axon_tui_runtime::{
    AgentTuiInputResult, AgentTuiRunOutput, AgentTuiRuntimeEvent, AgentTuiSubmit,
    AgentTuiSubmitFailure, RuntimeError,
};

pub(super) fn run_tui_control_command(
    submission: &AgentTuiSubmit,
    _config_path: Option<&Path>,
    _emit: &mut dyn FnMut(AgentTuiRuntimeEvent),
) -> Option<Result<AgentTuiRunOutput, AgentTuiSubmitFailure>> {
    match &submission.input {
        AgentTuiInputResult::ControlCommand { command, .. } => {
            Some(Err(AgentTuiSubmitFailure::without_output(
                RuntimeError::runtime(format!("未知 TUI 控制命令：{command}")),
            )))
        }
        _ => None,
    }
}

pub(crate) fn active_tui_session_id(submission: &AgentTuiSubmit) -> Option<String> {
    submission
        .overrides
        .session
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}
