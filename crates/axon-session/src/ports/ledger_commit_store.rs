//! Work Ledger 原子提交端口。

use axon_domain::{ExecutionRecord, GoalRecord, JarvisEvent, ResultRecord};
use std::fmt;

/// Work Ledger 提交错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerCommitError {
    /// 记录不存在。
    NotFound(String),
    /// lease、revision、状态或幂等冲突。
    Conflict(String),
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for LedgerCommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(detail) => write!(formatter, "Work Ledger 记录不存在：{detail}"),
            Self::Conflict(detail) => write!(formatter, "Work Ledger 提交冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Work Ledger 提交失败：{detail}"),
        }
    }
}

impl std::error::Error for LedgerCommitError {}

/// Execution 结算事实；由持久化层与 Session 事件及下一阶段一起原子提交。
pub struct ExecutionSettlement {
    /// Execution 的结算状态。
    pub execution: ExecutionRecord,
    /// 持有 lease 的执行者。
    pub owner: String,
    /// 本次 lease 的 fencing token。
    pub lease_token: String,
    /// 可复查的结算结果。
    pub result: ResultRecord,
    /// 待提交的 Goal 新状态。
    pub goal: GoalRecord,
    /// 乐观锁所需的旧 revision。
    pub expected_goal_revision: u64,
    /// 与结算同事务追加的事件。
    pub event: JarvisEvent,
}

/// Work Ledger 原子提交端口。
pub trait LedgerCommitStore: Send + Sync {
    /// 在同一事务中提交 Execution 终态、Result、Goal revision 与 Event。
    fn commit_execution(
        &self,
        execution: &ExecutionRecord,
        owner: &str,
        lease_token: &str,
        result: &ResultRecord,
        goal: &GoalRecord,
        expected_goal_revision: u64,
        event: &JarvisEvent,
    ) -> Result<(), LedgerCommitError>;
}
