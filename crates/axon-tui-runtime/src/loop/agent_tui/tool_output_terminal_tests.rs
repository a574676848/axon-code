use axon_domain::ToolExecutionPermission;

use super::{
    super::{tool_execution_started_line, tool_result_line},
    tool_receipt,
};
use crate::{
    AgentTuiToolReceiptDetails,
    r#loop::agent_tui::run_output::{AgentTuiToolExecutionView, AgentTuiToolKind},
    surface::cell::RenderCellData,
};

#[test]
fn write_stdin_projects_background_terminal_wait_lifecycle() {
    let started = tool_execution_started_line(&AgentTuiToolExecutionView {
        part_id: "tool-part:terminal-wait".to_owned(),
        request_id: 10,
        provider_call_id: "call-terminal-wait".to_owned(),
        tool_name: "builtin.write_stdin".to_owned(),
        kind: "shell".to_owned(),
        semantic_kind: AgentTuiToolKind::Shell,
        arguments_preview: r#"{"session_id":"session-1"}"#.to_owned(),
        intent: None,
        execution_permission: ToolExecutionPermission::Allowed,
    });
    let Some(RenderCellData::Exec(started_cell)) = started.data.as_ref() else {
        panic!("write_stdin start should render as exec data");
    };
    assert_eq!(started_cell.display_label, "等待终端");

    let completed = tool_result_line(&tool_receipt(
        "call-terminal-wait",
        "builtin.write_stdin",
        AgentTuiToolReceiptDetails {
            kind: AgentTuiToolKind::Shell,
            status: "completed".to_owned(),
            summary: "shell completed".to_owned(),
            metadata: Some(serde_json::json!({
                "argumentsPreview": "cargo test --workspace",
                "terminalInteraction": {"kind": "wait", "stdin": ""}
            })),
            error_type: None,
            filesystem_write_performed: false,
        },
    ));
    let Some(RenderCellData::Exec(completed_cell)) = completed.data.as_ref() else {
        panic!("write_stdin completion should render as exec data");
    };
    assert_eq!(completed_cell.display_label, "已等待终端");
    assert_eq!(
        completed_cell.display_target.as_deref(),
        Some("cargo test --workspace")
    );
    assert_eq!(completed_cell.arguments_preview, None);
}
