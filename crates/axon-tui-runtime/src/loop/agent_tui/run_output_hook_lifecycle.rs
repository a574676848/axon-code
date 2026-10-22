//! TUI Hook 生命周期状态模型。

use super::run_output_lifecycle::{AgentTuiHistoryItem, AgentTuiHistoryLine};

const HOOK_HISTORY_TITLE: &str = "Hook";
const HOOK_STATUS_FAILED_LABEL: &str = "失败";
const HOOK_STATUS_BLOCKED_LABEL: &str = "阻断";
const HOOK_STATUS_STOPPED_LABEL: &str = "已停止";
const HOOK_STATUS_RUNNING_LABEL: &str = "运行中";
const HOOK_STATUS_PENDING_LABEL: &str = "等待";

/// TUI Hook 状态视图。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiHookStatusView {
    /// Hook 运行稳定 ID，用于配对 start/end。
    pub run_id: String,
    /// Hook 名称。
    pub name: String,
    /// Hook 状态。
    pub status: AgentTuiHookStatus,
    /// Hook 运行态短提示。
    pub status_message: Option<String>,
    /// Hook 输出条目。
    pub entries: Vec<AgentTuiHookOutputEntry>,
}

/// TUI Hook 输出条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiHookOutputEntry {
    /// 输出类型。
    pub kind: AgentTuiHookOutputKind,
    /// 输出内容。
    pub text: String,
}

/// TUI Hook 输出类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiHookOutputKind {
    /// 普通提示。
    Feedback,
    /// 警告。
    Warning,
    /// 阻断说明。
    Stop,
    /// 上下文注入。
    Context,
    /// 错误。
    Error,
}

/// TUI Hook 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTuiHookStatus {
    /// Hook 已完成。
    Completed,
    /// Hook 执行失败。
    Failed,
    /// Hook 正在执行。
    Running,
    /// Hook 等待执行。
    Pending,
    /// Hook 被阻塞。
    Blocked,
    /// Hook 已停止。
    Stopped,
    /// 未识别状态。
    Unknown,
}

impl AgentTuiHookStatusView {
    /// 创建 Hook 状态视图。
    #[must_use]
    pub fn new(name: impl Into<String>, status: impl AsRef<str>, detail: Option<String>) -> Self {
        let name = name.into();
        Self::with_run_id(name.clone(), name, status, detail)
    }

    /// 创建带稳定运行 ID 的 Hook 状态视图。
    #[must_use]
    pub fn with_run_id(
        run_id: impl Into<String>,
        name: impl Into<String>,
        status: impl AsRef<str>,
        detail: Option<String>,
    ) -> Self {
        let name = normalize_text_or_default(name.into(), "Hook");
        let status_message = detail.and_then(|value| {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_owned())
        });
        Self {
            run_id: normalize_hook_run_id(run_id.into(), &name),
            name,
            status: AgentTuiHookStatus::from_raw(status.as_ref()),
            status_message,
            entries: Vec::new(),
        }
    }

    /// 创建带结构化输出的 Hook 状态视图。
    #[must_use]
    pub fn with_entries(
        name: impl Into<String>,
        status: impl AsRef<str>,
        status_message: Option<String>,
        entries: Vec<AgentTuiHookOutputEntry>,
    ) -> Self {
        let name = name.into();
        Self::with_run_id_and_entries(name.clone(), name, status, status_message, entries)
    }

    /// 创建带稳定运行 ID 与结构化输出的 Hook 状态视图。
    #[must_use]
    pub fn with_run_id_and_entries(
        run_id: impl Into<String>,
        name: impl Into<String>,
        status: impl AsRef<str>,
        status_message: Option<String>,
        entries: Vec<AgentTuiHookOutputEntry>,
    ) -> Self {
        let name = normalize_text_or_default(name.into(), "Hook");
        Self {
            run_id: normalize_hook_run_id(run_id.into(), &name),
            name,
            status: AgentTuiHookStatus::from_raw(status.as_ref()),
            status_message: normalize_optional_text(status_message),
            entries,
        }
    }

    /// 转换为可持久化的简短历史行，用于运行期 transcript 与 resume replay。
    #[must_use]
    pub fn persistent_history_line(&self) -> AgentTuiHistoryLine {
        AgentTuiHistoryLine::with_id(
            format!("history:hook:{}", self.run_id),
            HOOK_HISTORY_TITLE,
            vec![AgentTuiHistoryItem::new(
                self.persistent_summary(),
                Some(self.name.clone()),
                self.persistent_status_label().map(ToOwned::to_owned),
            )],
        )
    }

    /// 判断当前 Hook 是否值得进入持久 transcript/history。
    #[must_use]
    pub fn should_persist_history_line(&self) -> bool {
        match self.status {
            AgentTuiHookStatus::Completed => self
                .status_message
                .as_deref()
                .is_some_and(|message| !message.trim().is_empty()),
            AgentTuiHookStatus::Failed
            | AgentTuiHookStatus::Blocked
            | AgentTuiHookStatus::Stopped => true,
            AgentTuiHookStatus::Running | AgentTuiHookStatus::Pending => self
                .status_message
                .as_deref()
                .is_some_and(|message| !message.trim().is_empty()),
            AgentTuiHookStatus::Unknown => false,
        }
    }

    fn persistent_summary(&self) -> String {
        if let Some(message) = self
            .status_message
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return message.to_owned();
        }
        if self.status != AgentTuiHookStatus::Completed
            && let Some(entry) = self.entries.iter().find(|entry| {
                matches!(
                    entry.kind,
                    AgentTuiHookOutputKind::Feedback
                        | AgentTuiHookOutputKind::Warning
                        | AgentTuiHookOutputKind::Stop
                        | AgentTuiHookOutputKind::Error
                ) && !entry.text.trim().is_empty()
            })
        {
            return entry.text.trim().to_owned();
        }
        self.name.clone()
    }

    fn persistent_status_label(&self) -> Option<&'static str> {
        match self.status {
            AgentTuiHookStatus::Completed => None,
            AgentTuiHookStatus::Failed => Some(HOOK_STATUS_FAILED_LABEL),
            AgentTuiHookStatus::Blocked => Some(HOOK_STATUS_BLOCKED_LABEL),
            AgentTuiHookStatus::Stopped => Some(HOOK_STATUS_STOPPED_LABEL),
            AgentTuiHookStatus::Running => Some(HOOK_STATUS_RUNNING_LABEL),
            AgentTuiHookStatus::Pending => Some(HOOK_STATUS_PENDING_LABEL),
            AgentTuiHookStatus::Unknown => None,
        }
    }
}

impl AgentTuiHookOutputEntry {
    /// 创建 Hook 输出条目。
    #[must_use]
    pub fn new(kind: AgentTuiHookOutputKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }

    /// 创建自动分类的 Hook 输出条目。
    #[must_use]
    pub fn from_status_text(status: AgentTuiHookStatus, text: impl Into<String>) -> Option<Self> {
        let text = normalize_text(text.into())?;
        Some(Self::new(AgentTuiHookOutputKind::from_status(status), text))
    }
}

impl AgentTuiHookOutputKind {
    const fn from_status(status: AgentTuiHookStatus) -> Self {
        match status {
            AgentTuiHookStatus::Failed => Self::Error,
            AgentTuiHookStatus::Blocked => Self::Stop,
            AgentTuiHookStatus::Stopped => Self::Stop,
            AgentTuiHookStatus::Completed => Self::Feedback,
            AgentTuiHookStatus::Running
            | AgentTuiHookStatus::Pending
            | AgentTuiHookStatus::Unknown => Self::Feedback,
        }
    }
}

impl AgentTuiHookStatus {
    /// 判断 Hook 是否已进入终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Blocked | Self::Stopped
        )
    }

    /// 从运行时或 JSON 边界的原始状态值归一为 TUI Hook 状态。
    #[must_use]
    pub fn from_raw(status: &str) -> Self {
        match status {
            "completed" | "success" | "loaded" => Self::Completed,
            "failed" | "error" => Self::Failed,
            "running" | "calling" | "active" => Self::Running,
            "pending" | "queued" => Self::Pending,
            "blocked" => Self::Blocked,
            "stopped" | "interrupted" | "cancelled" | "canceled" => Self::Stopped,
            _ => Self::Unknown,
        }
    }
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(normalize_text)
}

fn normalize_text(value: String) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn normalize_hook_run_id(run_id: String, fallback: &str) -> String {
    normalize_text(run_id).unwrap_or_else(|| fallback.to_owned())
}

fn normalize_text_or_default(value: String, fallback: &str) -> String {
    normalize_text(value).unwrap_or_else(|| fallback.to_owned())
}
