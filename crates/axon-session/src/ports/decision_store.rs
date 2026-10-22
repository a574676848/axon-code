//! Work Ledger Decision 持久化端口。

use axon_domain::{DecisionRecord, ScopeRef};
use std::fmt;

/// Decision 存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionStoreError {
    /// 冲突。
    Conflict(String),
    /// 存储失败。
    Storage(String),
}

impl fmt::Display for DecisionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(detail) => write!(formatter, "Decision 冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Decision 存储失败：{detail}"),
        }
    }
}

impl std::error::Error for DecisionStoreError {}

/// Decision 持久化端口。
pub trait DecisionStore: Send + Sync {
    /// 按作用域 revision 追加 Decision。
    fn append_decision(
        &self,
        decision: &DecisionRecord,
        expected_revision: u64,
    ) -> Result<(), DecisionStoreError>;

    /// 读取作用域最新 Decision；过期和撤销由领域模型 fail-closed。
    fn get_latest_decision(
        &self,
        scope: &ScopeRef,
        now_ms: u64,
    ) -> Result<Option<DecisionRecord>, DecisionStoreError>;
}
