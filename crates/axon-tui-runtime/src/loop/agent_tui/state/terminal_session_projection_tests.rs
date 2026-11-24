//! 后台终端会话聚合投影测试。

use super::{AgentTuiState, runtime_flow_test_support::*};
use crate::r#loop::agent_tui::run_output::{
    AgentTuiToolCompletionView, AgentTuiToolExecutionView, AgentTuiToolKind,
    AgentTuiToolOutputChunk,
};
use crate::r#loop::agent_tui::wire_event_test_support::{
    tool_completed_event, tool_output_chunk_event, tool_started_event,
};
use crate::surface::cell::{RenderCellData, RenderCellKind};
use axon_domain::ToolExecutionPermission;

const TERMINAL_PART_ID: &str = "tool-part:terminal";
const TERMINAL_SESSION_ID: &str = "terminal-session-1";
const TERMINAL_COMMAND: &str = "cargo test --workspace";
const POWERSHELL_WRAPPED_COMMAND: &str = "pwsh -NoLogo -NoProfile -NonInteractive -Command try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {} cargo test --workspace";

#[test]
fn empty_terminal_polls_update_one_stable_cell_and_output_buffer() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行测试"));
    state.apply_runtime_event(tool_started_event(shell_start()));
    state.apply_runtime_event(tool_completed_event(shell_running()));

    apply_wait_poll(
        &mut state,
        "tool-part:wait-1",
        "call-wait-1",
        "checking",
        "running",
    );
    apply_wait_poll(
        &mut state,
        "tool-part:wait-2",
        "call-wait-2",
        "testing",
        "running",
    );
    apply_wait_poll(
        &mut state,
        "tool-part:wait-final",
        "call-wait-final",
        "test result: ok",
        "completed",
    );

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    let cell = state
        .cells
        .transcript
        .iter()
        .find_map(|record| match record.data.as_ref() {
            Some(RenderCellData::Exec(cell)) => Some(cell),
            _ => None,
        })
        .expect("应保留聚合后的 terminal cell");
    assert_eq!(cell.id, TERMINAL_PART_ID);
    assert_eq!(cell.display_label, "已等待终端");
    assert_eq!(cell.display_target.as_deref(), Some(TERMINAL_COMMAND));
    assert!(
        cell.output_preview
            .as_deref()
            .is_some_and(|output| output.contains("test result: ok"))
    );
}

#[test]
fn wait_poll_without_initial_session_mapping_reuses_latest_shell_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行测试"));
    state.apply_runtime_event(tool_started_event(shell_start()));
    let mut shell = shell_running();
    shell.metadata = Some(serde_json::json!({
        "status": "running",
        "argumentsPreview": POWERSHELL_WRAPPED_COMMAND,
        "outputPreview": "checking"
    }));
    state.apply_runtime_event(tool_completed_event(shell));

    apply_wait_poll(
        &mut state,
        "tool-part:wait-unmapped",
        "call-wait-unmapped",
        "checking",
        "running",
    );

    assert_eq!(
        cell_count(&state, RenderCellKind::EXEC),
        1,
        "transcript={:?}",
        state
            .cells
            .transcript
            .iter()
            .map(|line| (
                line.semantic_part_id(),
                line.provider_call_id(),
                line.data.clone()
            ))
            .collect::<Vec<_>>()
    );
    let cell = state
        .cells
        .transcript
        .iter()
        .find_map(|record| match record.data.as_ref() {
            Some(RenderCellData::Exec(cell)) => Some(cell),
            _ => None,
        })
        .expect("应复用原 shell cell");
    assert_eq!(cell.id, TERMINAL_PART_ID);
}

#[test]
fn wait_output_without_initial_session_mapping_reuses_shell_cell() {
    let mut state = test_state();
    state.start_runtime_submit(&test_submission("执行测试"));
    state.apply_runtime_event(tool_started_event(shell_start()));

    let wait_part_id = "tool-part:wait-output-only";
    state.apply_runtime_event(tool_started_event(AgentTuiToolExecutionView {
        part_id: wait_part_id.to_owned(),
        request_id: 2,
        provider_call_id: "call-wait-output-only".to_owned(),
        tool_name: "builtin.write_stdin".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        arguments_preview: format!(r#"{{"session_id":"{TERMINAL_SESSION_ID}"}}"#),
        intent: None,
        execution_permission: ToolExecutionPermission::Allowed,
    }));
    state.apply_runtime_event(tool_output_chunk_event(AgentTuiToolOutputChunk {
        part_id: wait_part_id.to_owned(),
        provider_call_id: "call-wait-output-only".to_owned(),
        tool_name: "builtin.write_stdin".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        stream: "stdout".to_owned(),
        sequence: 1,
        text: "checking".to_owned(),
    }));

    assert_eq!(cell_count(&state, RenderCellKind::EXEC), 1);
    let cell = state
        .cells
        .transcript
        .iter()
        .find_map(|record| match record.data.as_ref() {
            Some(RenderCellData::Exec(cell)) => Some(cell),
            _ => None,
        })
        .expect("应复用原 shell cell");
    assert_eq!(cell.id, TERMINAL_PART_ID);
    assert!(
        cell.output_preview
            .as_deref()
            .is_some_and(|output| output.contains("checking"))
    );
}

fn apply_wait_poll(
    state: &mut AgentTuiState,
    part_id: &str,
    provider_call_id: &str,
    output: &str,
    status: &str,
) {
    state.apply_runtime_event(tool_started_event(AgentTuiToolExecutionView {
        part_id: part_id.to_owned(),
        request_id: 2,
        provider_call_id: provider_call_id.to_owned(),
        tool_name: "builtin.write_stdin".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        arguments_preview: format!(r#"{{"session_id":"{TERMINAL_SESSION_ID}"}}"#),
        intent: None,
        execution_permission: ToolExecutionPermission::Allowed,
    }));
    state.apply_runtime_event(tool_output_chunk_event(AgentTuiToolOutputChunk {
        part_id: part_id.to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: "builtin.write_stdin".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        stream: "stdout".to_owned(),
        sequence: 1,
        text: output.to_owned(),
    }));
    state.apply_runtime_event(tool_completed_event(AgentTuiToolCompletionView {
        part_id: part_id.to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: "builtin.write_stdin".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        status: status.to_owned(),
        summary: output.to_owned(),
        metadata: Some(serde_json::json!({
            "status": status,
            "argumentsPreview": POWERSHELL_WRAPPED_COMMAND,
            "outputPreview": output,
            "sessionId": (status == "running").then_some(TERMINAL_SESSION_ID),
            "terminalInteraction": {"kind": "wait", "stdin": ""}
        })),
        error_type: None,
        filesystem_write_performed: false,
        duration_ms: Some(1),
    }));
}

fn shell_start() -> AgentTuiToolExecutionView {
    AgentTuiToolExecutionView {
        part_id: TERMINAL_PART_ID.to_owned(),
        request_id: 1,
        provider_call_id: "call-shell".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        arguments_preview: TERMINAL_COMMAND.to_owned(),
        intent: None,
        execution_permission: ToolExecutionPermission::Allowed,
    }
}

fn shell_running() -> AgentTuiToolCompletionView {
    AgentTuiToolCompletionView {
        part_id: TERMINAL_PART_ID.to_owned(),
        provider_call_id: "call-shell".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        status: "running".to_owned(),
        summary: "终端运行中".to_owned(),
        metadata: Some(serde_json::json!({
            "status": "running",
            "argumentsPreview": POWERSHELL_WRAPPED_COMMAND,
            "outputPreview": "checking",
            "sessionId": TERMINAL_SESSION_ID
        })),
        error_type: None,
        filesystem_write_performed: false,
        duration_ms: Some(1),
    }
}
