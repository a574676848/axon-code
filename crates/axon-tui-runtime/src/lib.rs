//! Axon Agent TUI 运行时入口。

#[path = "agent_pane/agent_pane_turn_event_store.rs"]
mod agent_pane_turn_event_store;
mod app_shell;
mod r#loop;
mod surface;
mod ui_workflow;
mod widgets;

use r#loop::agent_tui;
use ratatui::style::Color;

/// CLI 与 TUI 共用的终端语义色角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalTextStyleRole {
    /// 次要说明文本。
    Muted,
    /// 当前操作和标题。
    Accent,
    /// 成功、完成与已就绪状态。
    Success,
    /// 需要注意但可继续的状态。
    Warning,
    /// 取消、失败与错误状态。
    Error,
}

/// 返回与当前 TUI 调色板一致的 ANSI 前景色前缀。
///
/// `NO_COLOR` 时返回空字符串；调用方在文本后追加 `\x1b[0m` 即可。
#[must_use]
pub fn terminal_text_style_prefix(role: TerminalTextStyleRole) -> String {
    use app_shell::style_tokens::{TranscriptColorRole, transcript_palette};

    let role = match role {
        TerminalTextStyleRole::Muted => TranscriptColorRole::Muted,
        TerminalTextStyleRole::Accent => TranscriptColorRole::Accent,
        TerminalTextStyleRole::Success => TranscriptColorRole::ToolCompleted,
        TerminalTextStyleRole::Warning => TranscriptColorRole::Warning,
        TerminalTextStyleRole::Error => TranscriptColorRole::ToolFailed,
    };
    ansi_foreground(transcript_palette().color(role))
}

fn ansi_foreground(color: Color) -> String {
    match color {
        Color::Reset => String::new(),
        Color::Black => "\x1b[30m".to_owned(),
        Color::Red => "\x1b[31m".to_owned(),
        Color::Green => "\x1b[32m".to_owned(),
        Color::Yellow => "\x1b[33m".to_owned(),
        Color::Blue => "\x1b[34m".to_owned(),
        Color::Magenta => "\x1b[35m".to_owned(),
        Color::Cyan => "\x1b[36m".to_owned(),
        Color::Gray => "\x1b[37m".to_owned(),
        Color::DarkGray => "\x1b[90m".to_owned(),
        Color::LightRed => "\x1b[91m".to_owned(),
        Color::LightGreen => "\x1b[92m".to_owned(),
        Color::LightYellow => "\x1b[93m".to_owned(),
        Color::LightBlue => "\x1b[94m".to_owned(),
        Color::LightMagenta => "\x1b[95m".to_owned(),
        Color::LightCyan => "\x1b[96m".to_owned(),
        Color::White => "\x1b[97m".to_owned(),
        Color::Indexed(index) => format!("\x1b[38;5;{index}m"),
        Color::Rgb(red, green, blue) => format!("\x1b[38;2;{red};{green};{blue}m"),
    }
}

pub use agent_pane_turn_event_store::{
    AGENT_PANE_TURN_EVENTS_FILE, AgentPaneTurnEventRecord, AgentPaneTurnEventStoreRuntime,
};
pub use agent_tui::{
    AGENT_TUI_STATUS_EXITED, AGENT_TUI_STATUS_STARTED, AGENT_TUI_STATUS_SUBMITTED,
    AgentTuiAbilityCategorySnapshot, AgentTuiAbilityEntry, AgentTuiAbilityMatrixSnapshot,
    AgentTuiActiveTurn, AgentTuiCancelPayload, AgentTuiCancelToken, AgentTuiCatalog,
    AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState, AgentTuiCatalogLoader,
    AgentTuiChannelAttempt, AgentTuiCompletionInbox, AgentTuiCompletionInboxClient,
    AgentTuiCompletionNotification, AgentTuiConfigDiagnostic, AgentTuiConfigDiagnosticStatus,
    AgentTuiConfigStatus, AgentTuiDiffLine, AgentTuiError, AgentTuiHistoryItem,
    AgentTuiHistoryLine, AgentTuiHookOutputEntry, AgentTuiHookOutputKind, AgentTuiHookStatus,
    AgentTuiHookStatusView, AgentTuiInputElement, AgentTuiInputElementKind, AgentTuiInputResult,
    AgentTuiInterruptPayload, AgentTuiLaunchReport, AgentTuiLaunchRequest, AgentTuiLifecycleLine,
    AgentTuiLifecyclePhase, AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput,
    AgentTuiMcpAuthAction, AgentTuiMcpAuthLoader, AgentTuiMcpAuthOutcome, AgentTuiMcpAuthRequest,
    AgentTuiMcpServerCatalogItem, AgentTuiMessagePart, AgentTuiMessagePartRole,
    AgentTuiModelCatalogItem, AgentTuiPlanLine, AgentTuiPlanStatus, AgentTuiPlanStep,
    AgentTuiQuestionAnswer, AgentTuiQueuedAction, AgentTuiReasoningLine, AgentTuiReviewLine,
    AgentTuiReviewStatus, AgentTuiRunError, AgentTuiRunLine, AgentTuiRunOutput,
    AgentTuiRunOverrides, AgentTuiRunSessionInfo, AgentTuiRuntimeAdapter, AgentTuiRuntimeEvent,
    AgentTuiRuntimePayload, AgentTuiSessionCatalogItem, AgentTuiSessionSummary,
    AgentTuiSettingsChange, AgentTuiSkillStateChange, AgentTuiStatusItem, AgentTuiStatusLine,
    AgentTuiSteerResult, AgentTuiSubmit, AgentTuiSubmitFailure, AgentTuiTokenUsage,
    AgentTuiToolCompletionView, AgentTuiToolDiffSnapshotView, AgentTuiToolExecutionView,
    AgentTuiToolKind, AgentTuiToolLifecycleLine, AgentTuiToolLifecycleLineInput,
    AgentTuiToolOutputChunk, AgentTuiToolProgressView, AgentTuiToolReceipt,
    AgentTuiToolReceiptDetails, AgentTuiUsageLine, AgentTuiWorkspaceStatus,
    DaemonSubscriptionPhase, ToolDisplayProjection, axon_cli_runtime_event,
    cleanup_agent_tui_tmp_dir, ensure_agent_tui_tmp_cleanup_started, file_change_from_parts,
    launch_agent_tui_report, run_agent_tui_cli_with_runtime_adapter_and_settings,
    runtime_driver_event, tui_control_runtime_event, watcher_runtime_event,
};
pub use r#loop::agent_tui::state::viewport_state::ViewportScrollState;
pub use surface::cell_id::{CellId, CellIdAllocator};
pub use surface::cell_payload::{
    CellPayload, CellPayloadEnvelope, CellPayloadKind, CellPayloadSchemaError,
};
pub use surface::error_model::{
    CellPayloadError, IntoTuiError, SnapshotError, TuiError, TuiErrorAction, TuiErrorCellKind,
    TuiErrorSeverity, UiError,
};
pub use surface::event_envelope::{
    RUNTIME_EVENT_SCHEMA_VERSION, RuntimeCorrelationId, RuntimeEventEnvelope, RuntimeEventId,
    RuntimeEventSource, RuntimeEventTimestamp, RuntimeTransportTiming,
};
pub use surface::first_token_telemetry::{
    FirstTokenLatencyPercentiles, FirstTokenSample, FirstTokenTelemetrySnapshot,
};
pub use surface::mailbox_telemetry::{MailboxLaneTelemetrySnapshot, MailboxTelemetrySnapshot};
pub use surface::runtime_adapter::{
    RuntimeAdapterCapabilities, RuntimeAdapterContract, RuntimeAdapterSchemaVersion, RuntimeError,
    RuntimeErrorType, RuntimeEventEmitter, RuntimeEventSender,
};
pub use surface::runtime_payload::{RuntimePayload, RuntimePayloadKind};
pub use surface::telemetry::{TuiTelemetrySnapshot, TuiTraceSpanKind, TuiTracingSnapshot};
pub use surface::terminal_capabilities::TerminalCompatibilitySnapshot;
pub use surface::watchers::{
    RuntimeWatcherEvent, RuntimeWatcherPayload, RuntimeWatcherRefreshPayload, RuntimeWatcherSource,
    RuntimeWatcherStatus,
};
