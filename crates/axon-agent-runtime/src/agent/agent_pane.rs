//! Agent Pane 结构化投影。

use axon_domain::{
    ApplyExecutionStatus, ExecutionRequestId, PaneId, PaneSecurityBadge, PermissionSnapshotId,
    RuntimeCellSnapshot,
};

#[path = "agent_pane_command_context.rs"]
mod command_context;
#[path = "agent_pane_command_event.rs"]
mod command_event;
#[path = "agent_pane_explain.rs"]
mod explain;

pub use command_context::{CommandErrorKind, CommandErrorSpan, RecentCommandState};
use command_event::command_event_card;

/// Agent Pane 投影输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPaneProjectionInput {
    /// pane ID。
    pub pane_id: PaneId,
    /// 工作目录。
    pub cwd: String,
    /// shell kind。
    pub shell_kind: String,
    /// git state 摘要。
    pub git_state: Option<AgentPaneGitState>,
    /// 最近命令。
    pub recent_command: Option<RecentCommandState>,
    /// 权限快照。
    pub permission_snapshot_id: PermissionSnapshotId,
    /// 后端安全 badge。
    pub backend_badge: PaneSecurityBadge,
    /// 当前 RuntimeCell。
    pub runtime_cell: Option<RuntimeCellSnapshot>,
    /// 最新模型回答。
    pub response: Option<AgentResponseProjection>,
    /// Apply 执行结果投影。
    pub apply_execution: Option<AgentApplyExecutionProjection>,
}

/// Agent Pane apply 执行结果投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentApplyExecutionProjection {
    /// 绑定 RuntimeCell ID。
    pub cell_id: u64,
    /// 目标路径。
    pub path: String,
    /// 执行关联请求 ID。
    pub execution_request_id: ExecutionRequestId,
    /// 执行状态。
    pub status: ApplyExecutionStatus,
    /// 展示摘要。
    pub summary: String,
    /// 来源引用。
    pub source_ref: String,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 是否允许远控查看完整内容。
    pub remote_full_content_allowed: bool,
}

impl AgentApplyExecutionProjection {
    /// 构造 apply 执行结果投影。
    #[must_use]
    pub fn new(
        cell_id: u64,
        path: impl Into<String>,
        execution_request_id: ExecutionRequestId,
        status: ApplyExecutionStatus,
        summary: impl Into<String>,
        source_ref: impl Into<String>,
    ) -> Self {
        Self {
            cell_id,
            path: path.into(),
            execution_request_id,
            status,
            summary: summary.into(),
            source_ref: source_ref.into(),
            remote_trigger_allowed: false,
            remote_full_content_allowed: false,
        }
    }
}

/// Agent 模型回答投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponseProjection {
    /// 绑定 RuntimeCell ID。
    pub cell_id: u64,
    /// 回答预览。
    pub preview: String,
    /// 完整回答字符数。
    pub full_content_chars: usize,
    /// 预览是否已截断。
    pub truncated: bool,
    /// 回答来源引用。
    pub source_ref: String,
    /// 是否允许远控查看完整回答。
    pub remote_full_response_allowed: bool,
}

impl AgentResponseProjection {
    /// 从模型回答构造完整投影。
    #[must_use]
    pub fn from_content(cell_id: u64, content: &str, source_ref: impl Into<String>) -> Self {
        Self {
            cell_id,
            preview: content.to_owned(),
            full_content_chars: content.chars().count(),
            truncated: false,
            source_ref: source_ref.into(),
            remote_full_response_allowed: false,
        }
    }
}

/// git state 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPaneGitState {
    /// 分支名。
    pub branch: String,
    /// 是否有未提交变更。
    pub dirty: bool,
}

/// Agent Pane 结构化投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPaneProjection {
    /// pane ID。
    pub pane_id: PaneId,
    /// 可展示标题。
    pub title: String,
    /// 解释卡片。
    pub cards: Vec<AgentPaneCard>,
    /// 是否允许远控消费完整内容。
    pub remote_full_context_allowed: bool,
}

/// Agent Pane 卡片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentPaneCard {
    /// pane context。
    PaneContext {
        /// 工作目录。
        cwd: String,
        /// shell kind。
        shell_kind: String,
        /// 后端安全 badge 标签。
        backend_badge: String,
        /// 权限快照 ID。
        permission_snapshot_id: PermissionSnapshotId,
    },
    /// git state 摘要。
    GitState {
        /// 分支名。
        branch: String,
        /// 是否有未提交变更。
        dirty: bool,
    },
    /// 最近命令摘要。
    RecentCommand {
        /// 命令文本。
        command: String,
        /// 退出码。
        exit_code: Option<i32>,
    },
    /// 命令事件摘要。
    CommandEvent {
        /// 命令文本。
        command: String,
        /// 退出码。
        exit_code: Option<i32>,
        /// 展示摘要。
        summary: String,
        /// 来源引用。
        source_ref: String,
        /// 是否允许远控查看完整内容。
        remote_full_content_allowed: bool,
    },
    /// RuntimeCell 状态。
    RuntimeCell {
        /// cell 状态。
        status: String,
        /// 事件数量。
        event_count: usize,
    },
    /// 模型回答。
    ChatResponse {
        /// 绑定 RuntimeCell ID。
        cell_id: u64,
        /// 回答预览。
        preview: String,
        /// 预览是否已截断。
        truncated: bool,
        /// 回答来源引用。
        source_ref: String,
        /// 是否允许远控查看完整回答。
        remote_full_response_allowed: bool,
    },
    /// Apply 执行结果状态。
    ApplyExecution {
        /// 绑定 RuntimeCell ID。
        cell_id: u64,
        /// 目标路径。
        path: String,
        /// 执行关联请求 ID。
        execution_request_id: u64,
        /// 执行状态。
        status: ApplyExecutionStatus,
        /// 展示摘要。
        summary: String,
        /// 来源引用。
        source_ref: String,
        /// 是否允许远控触发。
        remote_trigger_allowed: bool,
        /// 是否允许远控查看完整内容。
        remote_full_content_allowed: bool,
    },
}

impl AgentPaneProjection {
    /// 从输入生成结构化投影。
    #[must_use]
    pub fn from_input(input: AgentPaneProjectionInput) -> Self {
        let mut cards = vec![AgentPaneCard::PaneContext {
            cwd: input.cwd.clone(),
            shell_kind: input.shell_kind.clone(),
            backend_badge: input.backend_badge.label.clone(),
            permission_snapshot_id: input.permission_snapshot_id,
        }];

        if let Some(git_state) = input.git_state {
            cards.push(AgentPaneCard::GitState {
                branch: git_state.branch,
                dirty: git_state.dirty,
            });
        }
        if let Some(recent_command) = input.recent_command {
            let command_event = command_event_card(input.pane_id, &recent_command);
            cards.push(AgentPaneCard::RecentCommand {
                command: recent_command.command,
                exit_code: recent_command.exit_code,
            });
            if let Some(card) = command_event {
                cards.push(card);
            }
        }
        if let Some(runtime_cell) = input.runtime_cell {
            cards.push(AgentPaneCard::RuntimeCell {
                status: format!("{:?}", runtime_cell.state),
                event_count: runtime_cell.event_count,
            });
        }
        if let Some(response) = input.response {
            cards.push(AgentPaneCard::ChatResponse {
                cell_id: response.cell_id,
                preview: response.preview,
                truncated: response.truncated,
                source_ref: response.source_ref,
                remote_full_response_allowed: response.remote_full_response_allowed,
            });
        }
        if let Some(apply_execution) = input.apply_execution {
            cards.push(AgentPaneCard::ApplyExecution {
                cell_id: apply_execution.cell_id,
                path: apply_execution.path,
                execution_request_id: apply_execution.execution_request_id.0,
                status: apply_execution.status,
                summary: apply_execution.summary,
                source_ref: apply_execution.source_ref,
                remote_trigger_allowed: apply_execution.remote_trigger_allowed,
                remote_full_content_allowed: apply_execution.remote_full_content_allowed,
            });
        }

        Self {
            pane_id: input.pane_id,
            title: "coding".to_owned(),
            cards,
            remote_full_context_allowed: false,
        }
    }
}

#[cfg(test)]
#[path = "agent_pane_tests.rs"]
mod tests;
