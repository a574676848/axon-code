//! Work Ledger Goal 领域模型。

use super::{ExecutionRecord, GoalId, JarvisEvent, ResultRecord, ScopeRef};
use serde::{Deserialize, Serialize};

/// Goal 文本字段的最大字符数。
pub const MAX_GOAL_TEXT_LENGTH: usize = 100_000;

/// Goal 生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    /// 尚未开始执行。
    Proposed,
    /// 可创建 Execution。
    Active,
    /// 等待用户或外部输入。
    Waiting,
    /// 暂停执行。
    Paused,
    /// 已成功完成。
    Completed,
    /// 已失败。
    Failed,
    /// 已取消。
    Cancelled,
}

impl GoalStatus {
    /// 判断是否为终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Goal 的持久计划内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalPlan {
    /// 用户可理解的目标。
    pub objective: String,
    /// 有序执行步骤。
    pub steps: Vec<String>,
}

/// Work Ledger 中的持久 Goal。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRecord {
    /// Goal 标识。
    pub goal_id: GoalId,
    /// 作用域。
    pub scope: ScopeRef,
    /// 父 Goal。
    pub parent_goal_id: Option<GoalId>,
    /// 简短标题。
    pub title: String,
    /// 当前状态。
    pub status: GoalStatus,
    /// 乐观并发版本。
    pub revision: u64,
    /// 计划内容。
    pub plan: GoalPlan,
    /// 验收条件。
    pub acceptance: Vec<String>,
    /// 可由事实支持的进度摘要。
    pub progress: Option<GoalProgress>,
    /// 等待原因。
    pub waiting_reason: Option<String>,
    /// 下一步。
    pub next_step: Option<String>,
    /// 调度优先级。
    #[serde(default)]
    pub priority: i32,
    /// 截止时间。
    pub deadline_ms: Option<u64>,
    /// 创建来源 Interaction。
    pub created_from_interaction_id: Option<String>,
    /// 取消原因。
    pub cancellation_reason: Option<String>,
    /// 取消请求时间。
    pub cancellation_requested_at_ms: Option<u64>,
    /// 创建时间。
    pub created_at_ms: u64,
    /// 更新时间。
    pub updated_at_ms: u64,
}

impl GoalRecord {
    /// 校验并创建 Goal。
    pub fn new(
        goal_id: GoalId,
        scope: ScopeRef,
        title: impl Into<String>,
        plan: GoalPlan,
        acceptance: Vec<String>,
        now_ms: u64,
    ) -> Result<Self, GoalError> {
        let title = bounded_text(title.into(), "title")?;
        let objective = bounded_text(plan.objective, "objective")?;
        let steps = plan
            .steps
            .into_iter()
            .map(|step| bounded_text(step, "step"))
            .collect::<Result<Vec<_>, _>>()?;
        let acceptance = acceptance
            .into_iter()
            .map(|item| bounded_text(item, "acceptance"))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            goal_id,
            scope,
            title,
            parent_goal_id: None,
            status: GoalStatus::Proposed,
            revision: 0,
            plan: GoalPlan { objective, steps },
            acceptance,
            progress: None,
            waiting_reason: None,
            next_step: None,
            priority: 0,
            deadline_ms: None,
            created_from_interaction_id: None,
            cancellation_reason: None,
            cancellation_requested_at_ms: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        })
    }

    /// 按合法迁移表变更状态并递增 revision。
    pub fn transition(&mut self, next: GoalStatus, now_ms: u64) -> Result<(), GoalError> {
        if self.status.is_terminal() || !valid_transition(self.status, next) {
            return Err(GoalError::InvalidTransition {
                from: self.status,
                to: next,
            });
        }
        self.status = next;
        self.revision = self.revision.saturating_add(1);
        self.updated_at_ms = now_ms;
        Ok(())
    }
}

fn valid_transition(from: GoalStatus, to: GoalStatus) -> bool {
    matches!(
        (from, to),
        (
            GoalStatus::Proposed,
            GoalStatus::Active | GoalStatus::Cancelled
        ) | (
            GoalStatus::Active,
            GoalStatus::Waiting
                | GoalStatus::Paused
                | GoalStatus::Completed
                | GoalStatus::Failed
                | GoalStatus::Cancelled
        ) | (
            GoalStatus::Waiting,
            GoalStatus::Active | GoalStatus::Cancelled
        ) | (
            GoalStatus::Paused,
            GoalStatus::Active | GoalStatus::Cancelled
        )
    )
}

fn bounded_text(value: String, field: &'static str) -> Result<String, GoalError> {
    if value.trim().is_empty() {
        return Err(GoalError::EmptyField(field));
    }
    if value.chars().count() > MAX_GOAL_TEXT_LENGTH {
        return Err(GoalError::FieldTooLong(field));
    }
    Ok(value)
}

/// Goal 校验或迁移错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalError {
    /// 字段为空。
    EmptyField(&'static str),
    /// 字段过长。
    FieldTooLong(&'static str),
    /// 状态迁移非法。
    InvalidTransition {
        /// 当前状态。
        from: GoalStatus,
        /// 目标状态。
        to: GoalStatus,
    },
}

impl std::fmt::Display for GoalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(formatter, "{field} 不能为空"),
            Self::FieldTooLong(field) => write!(formatter, "{field} 超过长度上限"),
            Self::InvalidTransition { from, to } => {
                write!(formatter, "不允许从 {from:?} 迁移到 {to:?}")
            }
        }
    }
}

impl std::error::Error for GoalError {}

/// Goal 查询作用域。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalQueryScope {
    /// 全局 Goal。
    Global,
    /// 当前 Workspace 绑定的 Goal。
    CurrentWorkspace,
    /// 全局 Goal 加当前 Workspace Goal。
    CurrentContext,
    /// 经受众和权限过滤后的全部可见 Goal。
    AllVisible,
}

/// Goal 创建作用域。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalCreationScope {
    /// 跨 Workspace 的个人事务或全局承诺。
    Global,
    /// 绑定创建时 Workspace 的项目事务。
    CurrentWorkspace,
}

/// Goal 创建命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalCreateCommand {
    /// 创建作用域。
    pub scope: GoalCreationScope,
    /// 简短标题。
    pub title: String,
    /// 可验收目标。
    pub objective: String,
    /// 有序执行步骤。
    pub steps: Vec<String>,
    /// 验收条件。
    pub acceptance: Vec<String>,
    /// 是否创建首个 queued Execution。
    pub create_execution: bool,
}

/// 可由事实支持的 Goal 进度摘要。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalProgress {
    /// 已完成步骤数。
    pub completed_step_count: u32,
    /// 总步骤数。
    pub total_step_count: u32,
    /// 当前步骤。
    pub current_step: Option<String>,
    /// 事实摘要。
    pub summary: Option<String>,
    /// 摘要更新时间。
    pub updated_at_ms: u64,
}

/// Goal 列表查询。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalListQuery {
    /// 查询作用域。
    pub scope: GoalQueryScope,
    /// 状态过滤；为空表示不按状态过滤。
    pub statuses: Vec<GoalStatus>,
    /// 单页上限。
    pub limit: u32,
    /// 绑定过滤器的游标。
    pub cursor: Option<String>,
}

/// Goal 详情查询。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalDetailQuery {
    /// Goal 标识。
    pub goal_id: GoalId,
    /// 是否包含最近 Execution。
    pub include_executions: bool,
    /// 是否包含时间线 Event。
    pub include_events: bool,
    /// 最近记录上限。
    pub limit: u32,
}

/// Goal 可变字段白名单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalUpdateCommand {
    /// 目标 Goal。
    pub goal_id: GoalId,
    /// 预期 revision。
    pub expected_revision: u64,
    /// 新标题。
    pub title: Option<String>,
    /// 新等待原因。
    pub waiting_reason: Option<String>,
    /// 下一步说明。
    pub next_step: Option<String>,
    /// 新优先级。
    pub priority: Option<i32>,
    /// 目标状态。
    pub status: Option<GoalStatus>,
}

/// Goal 取消命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalCancelCommand {
    /// 目标 Goal。
    pub goal_id: GoalId,
    /// 预期 revision。
    pub expected_revision: u64,
    /// 取消原因。
    pub reason: String,
}

/// Goal Activity 临时聚合视图。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalActivityView {
    /// Goal 当前事实。
    pub goal: GoalRecord,
    /// 事实进度。
    pub progress: Option<GoalProgress>,
    /// 最近 Execution。
    pub latest_execution: Option<ExecutionRecord>,
    /// 最近多次 Execution。
    pub recent_executions: Vec<ExecutionRecord>,
    /// 最近 Event。
    pub recent_events: Vec<JarvisEvent>,
    /// 最新 Result。
    pub latest_result: Option<ResultRecord>,
    /// 待处理审批标识。
    pub pending_decision: Option<String>,
    /// 下一页游标。
    pub next_cursor: Option<String>,
}

/// Goal 目标态领域错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCapabilityError {
    /// 游标与过滤器不匹配。
    CursorMismatch,
    /// 分页数量超出单次查询上限。
    InvalidLimit,
    /// revision 冲突。
    RevisionConflict {
        /// 调用方携带的 revision。
        expected: u64,
        /// 存储中的当前 revision。
        actual: u64,
    },
    /// 作用域不匹配。
    ScopeMismatch,
    /// 受众不可见。
    NotVisible,
    /// 缺少可变字段。
    EmptyUpdate,
    /// 优先级超出允许范围。
    InvalidPriority,
    /// 终态不可更新。
    TerminalGoal,
    /// active Execution 存在时禁止修改计划。
    ActiveExecutionPlanLocked,
    /// 取消与完成竞争。
    CancellationConflict,
    /// 取消原因为空。
    EmptyCancellationReason,
    /// 创建命令缺少非空目标、计划或验收条件。
    EmptyCreationPlan,
}

impl std::fmt::Display for GoalCapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CursorMismatch => formatter.write_str("Goal 分页游标与查询过滤器不匹配"),
            Self::InvalidLimit => formatter.write_str("Goal 分页数量超出单次查询上限"),
            Self::RevisionConflict { expected, actual } => {
                write!(
                    formatter,
                    "Goal revision 冲突：期望 {expected}，实际 {actual}"
                )
            }
            Self::ScopeMismatch => formatter.write_str("Goal 作用域与当前 Workspace 不匹配"),
            Self::NotVisible => formatter.write_str("Goal 对当前受众不可见"),
            Self::EmptyUpdate => formatter.write_str("Goal 更新至少需要一个可变字段"),
            Self::InvalidPriority => formatter.write_str("Goal priority 不能为负数"),
            Self::TerminalGoal => formatter.write_str("终态 Goal 不允许普通更新"),
            Self::ActiveExecutionPlanLocked => {
                formatter.write_str("存在 active Execution 时不允许修改计划")
            }
            Self::CancellationConflict => formatter.write_str("Goal 已完成，取消未生效"),
            Self::EmptyCancellationReason => formatter.write_str("取消原因不能为空"),
            Self::EmptyCreationPlan => {
                formatter.write_str("Goal 创建需要非空 title、objective、steps 和 acceptance")
            }
        }
    }
}

impl std::error::Error for GoalCapabilityError {}

impl GoalCreateCommand {
    /// 校验创建命令的目标、计划与验收条件。
    pub fn validate(&self) -> Result<(), GoalCapabilityError> {
        let has_text = |value: &str| !value.trim().is_empty();
        let has_items = |items: &[String]| {
            !items.is_empty() && items.iter().all(|item| !item.trim().is_empty())
        };
        if !has_text(&self.title)
            || !has_text(&self.objective)
            || !has_items(&self.steps)
            || !has_items(&self.acceptance)
        {
            return Err(GoalCapabilityError::EmptyCreationPlan);
        }
        Ok(())
    }
}

impl GoalListQuery {
    /// 校验查询边界。
    pub fn validate(&self, max_limit: u32) -> Result<(), GoalCapabilityError> {
        if self.limit == 0 || self.limit > max_limit {
            return Err(GoalCapabilityError::InvalidLimit);
        }
        Ok(())
    }
}

impl GoalUpdateCommand {
    /// 校验更新至少包含一个字段。
    pub fn validate(&self) -> Result<(), GoalCapabilityError> {
        if self.title.is_none()
            && self.waiting_reason.is_none()
            && self.next_step.is_none()
            && self.priority.is_none()
            && self.status.is_none()
        {
            return Err(GoalCapabilityError::EmptyUpdate);
        }
        if self.priority.is_some_and(|priority| priority < 0) {
            return Err(GoalCapabilityError::InvalidPriority);
        }
        Ok(())
    }
}

impl GoalCancelCommand {
    /// 校验取消原因。
    pub fn validate(&self) -> Result<(), GoalCapabilityError> {
        if self.reason.trim().is_empty() {
            return Err(GoalCapabilityError::EmptyCancellationReason);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_transition_increments_revision_and_rejects_terminal_changes() {
        let mut goal = GoalRecord::new(
            GoalId::new("goal-1").expect("ID"),
            ScopeRef::global(),
            "构建",
            GoalPlan {
                objective: "完成构建".to_owned(),
                steps: vec!["运行 cargo".to_owned()],
            },
            vec!["测试通过".to_owned()],
            1,
        )
        .expect("Goal");
        goal.transition(GoalStatus::Active, 2).expect("激活");
        goal.transition(GoalStatus::Completed, 3).expect("完成");
        assert_eq!(goal.revision, 2);
        assert!(goal.transition(GoalStatus::Active, 4).is_err());
    }

    #[test]
    fn goal_create_command_requires_plan_and_acceptance() {
        let valid = GoalCreateCommand {
            scope: GoalCreationScope::CurrentWorkspace,
            title: "修复构建".to_owned(),
            objective: "恢复主线构建".to_owned(),
            steps: vec!["定位失败用例".to_owned()],
            acceptance: vec!["工作区检查通过".to_owned()],
            create_execution: true,
        };
        assert!(valid.validate().is_ok());

        let empty_steps = GoalCreateCommand {
            steps: Vec::new(),
            ..valid.clone()
        };
        assert_eq!(
            empty_steps.validate().unwrap_err(),
            GoalCapabilityError::EmptyCreationPlan
        );

        let blank_acceptance = GoalCreateCommand {
            acceptance: vec!["  ".to_owned()],
            ..valid
        };
        assert_eq!(
            blank_acceptance.validate().unwrap_err(),
            GoalCapabilityError::EmptyCreationPlan
        );
    }
}
