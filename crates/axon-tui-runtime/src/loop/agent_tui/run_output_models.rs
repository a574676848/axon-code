//! TUI run output 基础模型。

use axon_display::normalize_display_units_in_text;

use crate::surface::error_model::{TuiError, TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity};

/// TUI 工具语义类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentTuiToolKind {
    /// 搜索类工具。
    Search,
    /// 文件读取工具。
    ReadFile,
    /// 路径发现工具。
    Glob,
    /// 记忆检索工具。
    MemorySearch,
    /// 记忆写入工具。
    MemoryWrite,
    /// 代际冷历史检索与回读工具。
    History,
    /// 会话私有便签工具。
    Notes,
    /// MCP 插件工具。
    Mcp,
    /// Skill 调用工具。
    InvokeSkill,
    /// Shell 命令工具。
    Shell,
    /// 统一文件编辑工具。
    Edit,
    /// 未识别工具。
    Unsupported,
}

/// TUI 工具执行回执。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiToolReceipt {
    /// 稳定 tool receipt part id。
    pub id: String,
    /// Provider returned tool call id。
    pub provider_call_id: String,
    /// 工具名称。
    pub tool_name: String,
    /// 工具语义类型。
    pub kind: AgentTuiToolKind,
    /// 执行状态。
    pub status: String,
    /// 工具结果摘要。
    pub summary: String,
    /// 工具结构化 metadata，用于渲染 diff、诊断和产物信息。
    pub metadata: Option<serde_json::Value>,
    /// 错误类型。
    pub error_type: Option<String>,
    /// 是否执行了文件系统写入。
    pub filesystem_write_performed: bool,
}

/// 工具回执的执行结果字段。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentTuiToolReceiptDetails {
    /// 工具类别。
    pub kind: AgentTuiToolKind,
    /// 执行状态。
    pub status: String,
    /// 摘要。
    pub summary: String,
    /// 可选结构化元数据。
    pub metadata: Option<serde_json::Value>,
    /// 可选错误类型。
    pub error_type: Option<String>,
    /// 是否执行了文件系统写入。
    pub filesystem_write_performed: bool,
}

/// TUI 运行会话信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiRunSessionInfo {
    /// 当前 run session id。
    pub session_id: String,
    /// 本轮恢复自哪个 session。
    pub resumed_from: Option<String>,
    /// 模型生成的会话标题。
    pub title: Option<String>,
}

/// TUI 运行错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiRunError {
    /// 稳定 error part id。
    pub id: String,
    /// 用户可读错误消息。
    pub message: String,
    /// 稳定运行时错误类型。
    pub error_type: Option<String>,
    /// 关联 HTTP 状态码。
    pub http_status: Option<u16>,
    /// provider/runtime 细分错误码。
    pub error_code: Option<String>,
    /// provider/runtime 关联参数。
    pub error_param: Option<String>,
    /// 导致该错误的 runtime event id。
    pub caused_by_runtime_event: Option<String>,
    /// 渠道尝试审计链，用于暴露 provider/model 级失败原因。
    pub channel_attempts: Vec<AgentTuiChannelAttempt>,
    /// TUI 错误严重级别。
    pub severity: TuiErrorSeverity,
    /// TUI 错误建议动作。
    pub action: TuiErrorAction,
    /// TUI 错误 cell 类型。
    pub cell_kind: TuiErrorCellKind,
    /// 结构化详情。
    pub details: Vec<String>,
    /// 错误专用恢复建议。
    pub next_step: Option<String>,
}

/// TUI 渠道尝试审计项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiChannelAttempt {
    /// 尝试的 provider 名称。
    pub provider: String,
    /// 尝试的 model id。
    pub model: String,
    /// provider 返回的 HTTP 状态码。
    pub http_status: Option<u16>,
    /// 尝试结果。
    pub outcome: String,
}

impl AgentTuiRunError {
    /// 创建带稳定 id 的普通文本错误。
    #[must_use]
    pub fn message_with_id(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            message: message.into(),
            error_type: None,
            http_status: None,
            error_code: None,
            error_param: None,
            caused_by_runtime_event: None,
            channel_attempts: Vec::new(),
            severity: TuiErrorSeverity::UserFacing,
            action: TuiErrorAction::InspectDetails,
            cell_kind: TuiErrorCellKind::Runtime,
            details: Vec::new(),
            next_step: None,
        }
    }

    /// 创建普通文本错误。
    #[cfg(test)]
    #[must_use]
    pub fn message(message: impl Into<String>) -> Self {
        Self::message_with_id("error:test:message", message)
    }

    /// 创建带稳定 id 的结构化运行错误。
    #[must_use]
    pub fn structured_with_id(
        id: impl Into<String>,
        message: impl Into<String>,
        error_type: Option<String>,
        http_status: Option<u16>,
        error_code: Option<String>,
        error_param: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            message: message.into(),
            error_type,
            http_status,
            error_code,
            error_param,
            caused_by_runtime_event: None,
            channel_attempts: Vec::new(),
            severity: TuiErrorSeverity::UserFacing,
            action: TuiErrorAction::InspectDetails,
            cell_kind: TuiErrorCellKind::Runtime,
            details: Vec::new(),
            next_step: None,
        }
    }

    /// 创建结构化运行错误。
    #[cfg(test)]
    #[must_use]
    pub fn structured(
        message: impl Into<String>,
        error_type: Option<String>,
        http_status: Option<u16>,
        error_code: Option<String>,
        error_param: Option<String>,
    ) -> Self {
        Self::structured_with_id(
            "error:test:structured",
            message,
            error_type,
            http_status,
            error_code,
            error_param,
        )
    }

    /// 从统一 TUI 错误模型创建运行错误。
    #[must_use]
    pub fn from_tui_error(error: TuiError) -> Self {
        Self {
            id: "error:tui".to_owned(),
            message: error.message,
            error_type: Some(error.error_type),
            http_status: None,
            error_code: error.error_code,
            error_param: None,
            caused_by_runtime_event: None,
            channel_attempts: Vec::new(),
            severity: error.severity,
            action: error.action,
            cell_kind: error.cell_kind,
            details: error.details,
            next_step: error.next_step,
        }
    }

    /// 附加 runtime event 追踪来源。
    #[must_use]
    pub fn with_caused_by_runtime_event(mut self, event_id: impl Into<String>) -> Self {
        let event_id = event_id.into();
        self.id = format!("error:{event_id}");
        self.caused_by_runtime_event = Some(event_id);
        self
    }

    /// 附加渠道尝试审计链。
    #[must_use]
    pub fn with_channel_attempts(mut self, channel_attempts: Vec<AgentTuiChannelAttempt>) -> Self {
        self.channel_attempts = channel_attempts;
        self
    }
}

impl AgentTuiRunSessionInfo {
    /// 创建 TUI run session 信息。
    #[must_use]
    pub fn new(session_id: impl Into<String>, resumed_from: Option<String>) -> Self {
        Self::with_title(session_id, resumed_from, None)
    }

    /// 创建携带标题的 TUI run session 信息。
    #[must_use]
    pub fn with_title(
        session_id: impl Into<String>,
        resumed_from: Option<String>,
        title: Option<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            resumed_from,
            title: title.and_then(|value| {
                let title = value.trim();
                (!title.is_empty()).then(|| title.to_owned())
            }),
        }
    }
}

impl AgentTuiToolReceipt {
    /// 创建带稳定 part id 的工具回执。
    #[must_use]
    pub fn with_id(
        id: impl Into<String>,
        provider_call_id: impl Into<String>,
        tool_name: impl Into<String>,
        details: AgentTuiToolReceiptDetails,
    ) -> Self {
        Self {
            id: id.into(),
            provider_call_id: provider_call_id.into(),
            tool_name: tool_name.into(),
            kind: details.kind,
            status: details.status,
            summary: normalize_display_units_in_text(&details.summary),
            metadata: details.metadata,
            error_type: details.error_type,
            filesystem_write_performed: details.filesystem_write_performed,
        }
    }
}

impl AgentTuiToolKind {
    /// 从稳定工具类型编码恢复 TUI 工具类型。
    #[must_use]
    pub fn from_code(value: &str) -> Self {
        match value {
            "search" => Self::Search,
            "read_file" => Self::ReadFile,
            "glob" => Self::Glob,
            "memory_search" => Self::MemorySearch,
            "memory_write" => Self::MemoryWrite,
            "history" => Self::History,
            "notes" => Self::Notes,
            "mcp" => Self::Mcp,
            "invoke_skill" => Self::InvokeSkill,
            "shell" => Self::Shell,
            "edit" => Self::Edit,
            _ => Self::Unsupported,
        }
    }

    /// 从规范工具名恢复 TUI 工具类型。
    #[must_use]
    pub fn from_tool_name(tool_name: &str) -> Self {
        if tool_name.starts_with("mcp__") {
            return Self::Mcp;
        }
        match tool_name {
            "builtin.search" | "search" => Self::Search,
            "builtin.read_file" | "read_file" => Self::ReadFile,
            "builtin.glob" | "glob" => Self::Glob,
            "builtin.memory_search" | "memory_search" => Self::MemorySearch,
            "builtin.memory_write" | "memory_write" => Self::MemoryWrite,
            "builtin.history_list_windows"
            | "builtin.history_list_items"
            | "builtin.history_read_item"
            | "builtin.history_search_contents" => Self::History,
            "builtin.notes_write_file"
            | "builtin.notes_append_to_file"
            | "builtin.notes_read_file"
            | "builtin.notes_list_files_by_prefix"
            | "builtin.notes_search_contents" => Self::Notes,
            "builtin.invoke_skill" | "invoke_skill" => Self::InvokeSkill,
            "builtin.shell_command" | "shell_command" | "builtin.write_stdin" | "write_stdin" => {
                Self::Shell
            }
            "builtin.edit" | "edit" => Self::Edit,
            _ => Self::Unsupported,
        }
    }

    /// 返回给终端用户看的 Codex 风格短英文标签。
    #[must_use]
    pub fn display_label(self) -> &'static str {
        crate::agent_tui::tool_display_projection::tool_kind_display_label(self)
    }
}
