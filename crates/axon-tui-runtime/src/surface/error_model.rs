//! TUI 错误分级与投影事实源。

use crate::surface::error_messages::{
    ErrorMessageInput, ErrorMessageKey, ErrorMessageProjection, project_error_message,
    project_error_message_for_key,
};
use std::fmt;

/// TUI 错误严重级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiErrorSeverity {
    /// 需要退出当前 TUI 的致命错误。
    Fatal,
    /// 用户需要看到并处理的错误。
    UserFacing,
    /// 可恢复错误，当前交互可继续。
    Recoverable,
    /// 内部错误，默认只暴露压缩摘要。
    Internal,
    /// 非阻断提示。
    Hint,
}

/// TUI 错误建议动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiErrorAction {
    /// 退出 TUI。
    Exit,
    /// 重新提交或稍后重试。
    Retry,
    /// 修复配置或重新 setup。
    Reconfigure,
    /// 新建会话。
    NewSession,
    /// 查看详细信息。
    InspectDetails,
    /// 无建议动作。
    None,
}

/// TUI 错误 cell 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TuiErrorCellKind {
    /// Runtime/adapter/provider 错误。
    Runtime,
    /// Snapshot 读写或 schema 错误。
    Snapshot,
    /// Cell payload schema 错误。
    CellPayload,
    /// UI workflow / 本地交互错误。
    Ui,
    /// 终端 I/O 错误。
    Terminal,
}

/// 可投影到 RenderViewport 的统一 TUI 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TuiError {
    /// 严重级别。
    pub severity: TuiErrorSeverity,
    /// 建议动作。
    pub action: TuiErrorAction,
    /// cell 类型。
    pub cell_kind: TuiErrorCellKind,
    /// 用户可见消息。
    pub message: String,
    /// 稳定错误类型。
    pub error_type: String,
    /// 可选错误码。
    pub error_code: Option<String>,
    /// 可选恢复建议。
    pub next_step: Option<String>,
    /// 结构化详情。
    pub details: Vec<String>,
}

/// 统一错误投影接口。
pub trait IntoTuiError {
    /// 转为 TUI 错误模型。
    fn into_tui_error(self) -> TuiError;
}

/// Snapshot 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotError {
    error: TuiError,
}

/// Cell payload 错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellPayloadError {
    error: TuiError,
}

/// UI 本地交互错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    error: TuiError,
}

impl TuiError {
    /// 创建 runtime 错误。
    #[must_use]
    pub fn runtime(message: impl Into<String>) -> Self {
        let message = message.into();
        Self::from_projection(project_error_message(&ErrorMessageInput::runtime(&message)))
    }

    /// 创建可恢复 steer 拒绝错误。
    #[must_use]
    pub fn steer_rejected(_message: impl Into<String>) -> Self {
        let mut error = Self::from_projection(project_error_message_for_key(
            ErrorMessageKey::RuntimeGeneric,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.severity = TuiErrorSeverity::Recoverable;
        error.action = TuiErrorAction::Retry;
        error.error_code = Some("steer_rejected".to_owned());
        error.next_step = Some("已保留为待重试输入".to_owned());
        error
    }

    /// 创建 adapter schema mismatch 错误。
    #[must_use]
    pub fn schema_mismatch(
        _message: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        let mut error = Self::from_projection(project_error_message_for_key(
            ErrorMessageKey::RuntimeSchemaMismatch,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("runtime_adapter_schema_mismatch".to_owned());
        error.details = vec![
            format!("期望 {}", expected.into()),
            format!("实际 {}", actual.into()),
        ];
        error
    }

    /// 创建 worker panic 错误。
    #[must_use]
    pub fn worker_panic(_panic_summary: impl Into<String>) -> Self {
        let mut error = Self::from_projection(project_error_message_for_key(
            ErrorMessageKey::RuntimeWorkerPanic,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("runtime_worker_panic".to_owned());
        error
    }

    /// 创建 worker channel 断开错误。
    #[must_use]
    pub fn worker_channel_disconnected() -> Self {
        let mut error = Self::from_projection(project_error_message_for_key(
            ErrorMessageKey::RuntimeGeneric,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("runtime_worker_channel_disconnected".to_owned());
        error.details = vec!["worker channel disconnected before finish".to_owned()];
        error.next_step = Some("运行通道已恢复，可重新提交本轮输入".to_owned());
        error
    }

    /// 创建终端 I/O 致命错误。
    #[must_use]
    pub fn terminal_io(_message: impl Into<String>) -> Self {
        let mut error = Self::from_projection(project_error_message_for_key(
            ErrorMessageKey::TerminalIo,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("terminal_io_failed".to_owned());
        error
    }

    fn from_projection(projection: ErrorMessageProjection) -> Self {
        Self {
            severity: projection.severity,
            action: projection.action,
            cell_kind: projection.cell_kind,
            message: projection.message,
            error_type: projection.key.as_str().to_owned(),
            error_code: None,
            next_step: projection.next_step,
            details: Vec::new(),
        }
    }

    /// 返回现有 TUI error_type。
    #[must_use]
    pub fn as_tui_error_type(&self) -> &str {
        &self.error_type
    }

    /// 返回 severity 稳定编码。
    #[must_use]
    pub const fn severity_code(&self) -> &'static str {
        self.severity.as_code()
    }

    /// 返回 action 稳定编码。
    #[must_use]
    pub const fn action_code(&self) -> &'static str {
        self.action.as_code()
    }

    /// 返回 cell kind 稳定编码。
    #[must_use]
    pub const fn cell_kind_code(&self) -> &'static str {
        self.cell_kind.as_code()
    }
}

impl SnapshotError {
    /// Snapshot 文件损坏。
    #[must_use]
    pub fn corrupt(path: impl Into<String>) -> Self {
        let mut error = TuiError::from_projection(project_error_message_for_key(
            ErrorMessageKey::SnapshotCorrupt,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("snapshot_corrupt".to_owned());
        error.details = vec![format!("路径 {}", path.into())];
        Self { error }
    }

    /// Snapshot 文件权限不可用。
    #[must_use]
    pub fn permission(path: impl Into<String>) -> Self {
        let mut error = TuiError::from_projection(project_error_message_for_key(
            ErrorMessageKey::SnapshotPermission,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("snapshot_permission".to_owned());
        error.details = vec![format!("路径 {}", path.into())];
        Self { error }
    }
}

impl CellPayloadError {
    /// Cell payload 缺少必需字段。
    #[must_use]
    pub fn missing_field(cell_kind: impl Into<String>, field: impl Into<String>) -> Self {
        let mut error = TuiError::from_projection(project_error_message_for_key(
            ErrorMessageKey::CellPayloadMissingField,
            crate::surface::error_messages::ErrorLocale::Zh,
        ));
        error.error_code = Some("cell_payload_missing_field".to_owned());
        error.details = vec![
            format!("cell {}", cell_kind.into()),
            format!("field {}", field.into()),
        ];
        Self { error }
    }
}

impl UiError {
    /// UI 本地可恢复错误。
    #[must_use]
    pub fn recoverable(message: impl Into<String>) -> Self {
        Self {
            error: TuiError {
                severity: TuiErrorSeverity::Recoverable,
                action: TuiErrorAction::None,
                cell_kind: TuiErrorCellKind::Ui,
                message: message.into(),
                error_type: "ui".to_owned(),
                error_code: None,
                next_step: None,
                details: Vec::new(),
            },
        }
    }
}

impl IntoTuiError for TuiError {
    fn into_tui_error(self) -> TuiError {
        self
    }
}

impl IntoTuiError for SnapshotError {
    fn into_tui_error(self) -> TuiError {
        self.error
    }
}

impl IntoTuiError for CellPayloadError {
    fn into_tui_error(self) -> TuiError {
        self.error
    }
}

impl IntoTuiError for UiError {
    fn into_tui_error(self) -> TuiError {
        self.error
    }
}

impl TuiErrorSeverity {
    /// 稳定编码。
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Fatal => "fatal",
            Self::UserFacing => "user_facing",
            Self::Recoverable => "recoverable",
            Self::Internal => "internal",
            Self::Hint => "hint",
        }
    }
}

impl TuiErrorAction {
    /// 稳定编码。
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Exit => "exit",
            Self::Retry => "retry",
            Self::Reconfigure => "reconfigure",
            Self::NewSession => "new_session",
            Self::InspectDetails => "inspect_details",
            Self::None => "none",
        }
    }
}

impl TuiErrorCellKind {
    /// 稳定编码。
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Snapshot => "snapshot",
            Self::CellPayload => "cell_payload",
            Self::Ui => "ui",
            Self::Terminal => "terminal",
        }
    }
}

impl fmt::Display for TuiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_error_projects_user_facing_new_session_action() {
        let error = SnapshotError::corrupt("session.json").into_tui_error();

        assert_eq!(error.severity, TuiErrorSeverity::UserFacing);
        assert_eq!(error.action, TuiErrorAction::NewSession);
        assert_eq!(error.cell_kind, TuiErrorCellKind::Snapshot);
        assert_eq!(error.error_code.as_deref(), Some("snapshot_corrupt"));
    }

    #[test]
    fn snapshot_permission_projects_user_facing_reconfigure_action() {
        let error = SnapshotError::permission("session.json").into_tui_error();

        assert_eq!(error.severity, TuiErrorSeverity::UserFacing);
        assert_eq!(error.action, TuiErrorAction::Reconfigure);
        assert_eq!(error.cell_kind, TuiErrorCellKind::Snapshot);
        assert_eq!(error.error_code.as_deref(), Some("snapshot_permission"));
    }

    #[test]
    fn cell_payload_error_projects_internal_inspection_action() {
        let error = CellPayloadError::missing_field("Diff", "changes").into_tui_error();

        assert_eq!(error.severity, TuiErrorSeverity::Internal);
        assert_eq!(error.action, TuiErrorAction::InspectDetails);
        assert_eq!(error.cell_kind, TuiErrorCellKind::CellPayload);
        assert!(error.details.iter().any(|detail| detail == "field changes"));
    }

    #[test]
    fn ui_error_projects_recoverable_noop_action() {
        let error = UiError::recoverable("dock already closed").into_tui_error();

        assert_eq!(error.severity, TuiErrorSeverity::Recoverable);
        assert_eq!(error.action, TuiErrorAction::None);
        assert_eq!(error.cell_kind, TuiErrorCellKind::Ui);
    }
}
