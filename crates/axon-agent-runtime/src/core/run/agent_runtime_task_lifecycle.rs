//! Runtime task 生命周期合同。

use serde::{Deserialize, Serialize};

use crate::agent_turn_runtime::AgentTurnEventKind;

/// Runtime task 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeTaskKind {
    /// 普通模型 turn。
    RegularTurn,
    /// 上下文压缩任务。
    Compact,
    /// 审阅或审批任务。
    Review,
    /// 用户显式触发的 shell 执行任务。
    UserShell,
}

impl RuntimeTaskKind {
    /// 根据 turn event 事实推导 task 类型。
    #[must_use]
    pub fn from_turn_event(kind: AgentTurnEventKind, tool_name: Option<&str>) -> Self {
        if matches!(
            kind,
            AgentTurnEventKind::CompactionStarted | AgentTurnEventKind::CompactionCompleted
        ) {
            return Self::Compact;
        }
        if matches!(kind, AgentTurnEventKind::AwaitingExternal) {
            return Self::Review;
        }
        if matches!(
            kind,
            AgentTurnEventKind::ToolExecutionStarted | AgentTurnEventKind::ToolCallCompleted
        ) && tool_name.is_some_and(is_shell_tool_name)
        {
            return Self::UserShell;
        }
        Self::RegularTurn
    }
}

/// Runtime task outcome 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RuntimeTaskOutcomeStatus {
    /// 任务正在运行。
    Running,
    /// 任务已完成。
    Completed,
    /// 任务失败。
    Failed,
    /// 任务被中断。
    Aborted,
    /// 任务被取消。
    Cancelled,
}

/// Runtime task outcome。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTaskOutcome {
    /// outcome 状态。
    pub status: RuntimeTaskOutcomeStatus,
    /// 可审计原因。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 任务耗时，单位毫秒。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl RuntimeTaskOutcome {
    /// 创建运行中 outcome。
    #[must_use]
    pub const fn running() -> Self {
        Self {
            status: RuntimeTaskOutcomeStatus::Running,
            reason: None,
            duration_ms: None,
        }
    }

    /// 根据 turn event 事实推导 outcome。
    #[must_use]
    pub fn from_turn_event(
        kind: AgentTurnEventKind,
        status: Option<&str>,
        reason: Option<&str>,
        duration_ms: Option<u64>,
    ) -> Self {
        let outcome_status = status
            .and_then(status_from_text)
            .unwrap_or_else(|| status_from_event_kind(kind));
        Self {
            status: outcome_status,
            reason: reason
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            duration_ms,
        }
    }

    /// 设置耗时。
    #[must_use]
    pub const fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }
}

fn status_from_text(status: &str) -> Option<RuntimeTaskOutcomeStatus> {
    match status.trim().to_ascii_lowercase().as_str() {
        "running" | "retrying" | "pending" | "waiting" => Some(RuntimeTaskOutcomeStatus::Running),
        "completed" | "complete" | "passed" | "success" | "succeeded" => {
            Some(RuntimeTaskOutcomeStatus::Completed)
        }
        "failed" | "failure" | "error" | "denied" | "blocked" => {
            Some(RuntimeTaskOutcomeStatus::Failed)
        }
        "aborted" | "interrupted" | "stopped" => Some(RuntimeTaskOutcomeStatus::Aborted),
        "cancelled" | "canceled" => Some(RuntimeTaskOutcomeStatus::Cancelled),
        _ => None,
    }
}

fn status_from_event_kind(kind: AgentTurnEventKind) -> RuntimeTaskOutcomeStatus {
    match kind {
        AgentTurnEventKind::Completed
        | AgentTurnEventKind::ProviderCompleted
        | AgentTurnEventKind::ToolResultsRecorded
        | AgentTurnEventKind::CompactionCompleted
        | AgentTurnEventKind::ReasoningCompleted
        | AgentTurnEventKind::ToolCallCompleted => RuntimeTaskOutcomeStatus::Completed,
        AgentTurnEventKind::Failed => RuntimeTaskOutcomeStatus::Failed,
        AgentTurnEventKind::Interrupted => RuntimeTaskOutcomeStatus::Aborted,
        AgentTurnEventKind::TurnStarted
        | AgentTurnEventKind::StreamingStarted
        | AgentTurnEventKind::FirstTokenReceived
        | AgentTurnEventKind::ReasoningStarted
        | AgentTurnEventKind::ProviderRetry
        | AgentTurnEventKind::ToolExecutionStarted
        | AgentTurnEventKind::AwaitingExternal
        | AgentTurnEventKind::CompactionStarted => RuntimeTaskOutcomeStatus::Running,
    }
}

fn is_shell_tool_name(tool_name: &str) -> bool {
    matches!(
        tool_name.trim(),
        "builtin.shell"
            | "builtin.shell_command"
            | "builtin.write_stdin"
            | "shell"
            | "shell_command"
            | "write_stdin"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_events_use_compact_task_contract() {
        let outcome = RuntimeTaskOutcome::from_turn_event(
            AgentTurnEventKind::CompactionCompleted,
            Some("completed"),
            Some("压缩完成"),
            Some(250),
        );

        assert_eq!(
            RuntimeTaskKind::from_turn_event(AgentTurnEventKind::CompactionCompleted, None),
            RuntimeTaskKind::Compact
        );
        assert_eq!(outcome.status, RuntimeTaskOutcomeStatus::Completed);
        assert_eq!(outcome.reason.as_deref(), Some("压缩完成"));
        assert_eq!(outcome.duration_ms, Some(250));
    }

    #[test]
    fn shell_events_have_specific_task_kind() {
        assert_eq!(
            RuntimeTaskKind::from_turn_event(
                AgentTurnEventKind::ToolExecutionStarted,
                Some("builtin.shell_command"),
            ),
            RuntimeTaskKind::UserShell
        );
        assert_eq!(
            RuntimeTaskKind::from_turn_event(
                AgentTurnEventKind::ToolExecutionStarted,
                Some("builtin.search")
            ),
            RuntimeTaskKind::RegularTurn
        );
    }

    #[test]
    fn async_runtime_task_outcomes_cover_cancel_failure_and_cleanup_states() {
        let cancelled = RuntimeTaskOutcome::from_turn_event(
            AgentTurnEventKind::Interrupted,
            Some("cancelled"),
            Some("user requested cancellation"),
            Some(17),
        );
        let failed = RuntimeTaskOutcome::from_turn_event(
            AgentTurnEventKind::Failed,
            None,
            Some("provider transport failed"),
            Some(42),
        );
        let cleanup = RuntimeTaskOutcome::from_turn_event(
            AgentTurnEventKind::Interrupted,
            Some("stopped"),
            Some("runtime task cleanup completed"),
            Some(9),
        );

        assert_eq!(cancelled.status, RuntimeTaskOutcomeStatus::Cancelled);
        assert_eq!(
            cancelled.reason.as_deref(),
            Some("user requested cancellation")
        );
        assert_eq!(cancelled.duration_ms, Some(17));
        assert_eq!(failed.status, RuntimeTaskOutcomeStatus::Failed);
        assert_eq!(failed.reason.as_deref(), Some("provider transport failed"));
        assert_eq!(cleanup.status, RuntimeTaskOutcomeStatus::Aborted);
        assert_eq!(
            cleanup.reason.as_deref(),
            Some("runtime task cleanup completed")
        );
    }
}
