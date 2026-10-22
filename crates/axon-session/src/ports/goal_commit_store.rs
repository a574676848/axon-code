//! Goal 首次提交的持久化端口。

use axon_domain::{ExecutionRecord, GoalRecord, JarvisEvent};
use std::fmt;

/// Goal 首次提交错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommitError {
    /// 输入或幂等身份冲突。
    Conflict(String),
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for GoalCommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(detail) => write!(formatter, "Goal 首次提交冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Goal 首次提交失败：{detail}"),
        }
    }
}

impl std::error::Error for GoalCommitError {}

/// 将 Agent 的 Goal 决定原子落到账本。
pub trait GoalCommitStore: Send + Sync {
    /// 原子创建尚未启动的 Goal 与创建事件。
    fn commit_goal_proposal(
        &self,
        goal: &GoalRecord,
        event: &JarvisEvent,
    ) -> Result<(), GoalCommitError>;

    /// 原子创建 Goal、首个 queued Execution 与对应 Event。
    ///
    /// event.sequence 必须为 0，具体全局序号由持久化事务分配。
    fn commit_goal_start(
        &self,
        goal: &GoalRecord,
        execution: &ExecutionRecord,
        event: &JarvisEvent,
    ) -> Result<(), GoalCommitError>;

    /// 原子启动已有 Goal，递增 revision、创建 queued Execution 并追加事件。
    ///
    /// 默认实现明确拒绝，防止没有事务实现的存储后端静默产生部分写入。
    fn commit_existing_goal_start(
        &self,
        _goal: &GoalRecord,
        _expected_revision: u64,
        _execution: &ExecutionRecord,
        _event: &JarvisEvent,
    ) -> Result<(), GoalCommitError> {
        Err(GoalCommitError::Conflict(
            "当前存储未实现已有 Goal 的原子启动".to_owned(),
        ))
    }
}
