use super::*;
use crate::surface::error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity};

#[test]
fn terminal_io_error_projects_fatal_exit_boundary() {
    let error = AgentTuiError::Io(io::Error::other("draw failed"));
    let tui_error = error.to_tui_error();

    assert_eq!(error.exit_code(), AGENT_TUI_TERMINAL_IO_EXIT_CODE);
    assert_eq!(tui_error.severity, TuiErrorSeverity::Fatal);
    assert_eq!(tui_error.action, TuiErrorAction::Exit);
    assert_eq!(tui_error.cell_kind, TuiErrorCellKind::Terminal);
    assert_eq!(tui_error.error_code.as_deref(), Some("terminal_io_failed"));
}
