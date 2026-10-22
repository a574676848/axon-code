//! Error cell view model。

use crate::{
    surface::cell_event_data::AgentRenderErrorCell,
    surface::error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity},
};

/// Error cell 渲染前的结构化视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ErrorCellView<'a> {
    pub(crate) title: &'a str,
    pub(crate) message: &'a str,
    pub(crate) details: &'a [String],
    pub(crate) next_step: Option<&'a str>,
    pub(crate) severity: TuiErrorSeverity,
    pub(crate) action: TuiErrorAction,
    pub(crate) cell_kind: TuiErrorCellKind,
}

impl<'a> ErrorCellView<'a> {
    pub(crate) fn from_cell(cell: &'a AgentRenderErrorCell) -> Self {
        Self {
            title: &cell.title,
            message: &cell.message,
            details: &cell.details,
            next_step: cell.next_step.as_deref(),
            severity: cell.severity,
            action: cell.action,
            cell_kind: cell.cell_kind,
        }
    }
}
