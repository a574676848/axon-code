//! Work Ledger Result 的查询与读取确认持久化端口。

use axon_domain::{ExecutionId, ResultId, ResultRecord, ScopeRef};
use std::fmt;

/// Result 存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultStoreError {
    /// 存储失败。
    Storage(String),
}

impl fmt::Display for ResultStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(detail) => write!(formatter, "Result 存储失败：{detail}"),
        }
    }
}

impl std::error::Error for ResultStoreError {}

/// Result 读取确认来源：用户真实读取。
pub const RESULT_ACK_SOURCE_READ: &str = "read";
/// Result 读取确认来源：完成通知隔天未读自动失效。
pub const RESULT_ACK_SOURCE_EXPIRED: &str = "expired";

/// Result 读取端口。
pub trait ResultStore: Send + Sync {
    /// 按作用域读取未确认 Result，结果数量有界且按产生时间升序。
    fn list_unread_results(
        &self,
        scope: &ScopeRef,
        limit: u32,
    ) -> Result<Vec<ResultRecord>, ResultStoreError>;

    /// 按 Execution 读取其唯一 Result；不存在时返回 `None`。
    ///
    /// 完成通知必须拿到本次执行的权威结论，不能退化成「扫描最近未读结果」，
    /// 否则同一 Workspace 的并发结果会互相挤掉。
    fn result_for_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<Option<ResultRecord>, ResultStoreError>;

    /// 按作用域确认一批 Result，返回本次首次确认的数量。
    ///
    /// 不存在、属于其他作用域或已确认的 Result 均不会产生副作用；
    /// 重复调用与批量中的重复 ID 都是幂等的。
    fn acknowledge_results(
        &self,
        scope: &ScopeRef,
        result_ids: &[ResultId],
        acknowledged_at_ms: u64,
    ) -> Result<u32, ResultStoreError>;

    /// 把一批隔天未读、随完成通知一起失效的 Result 标记为过期确认。
    ///
    /// 与 `acknowledge_results` 的区别只体现在确认来源：用户从未真正读到这条
    /// 结论，因此账本必须能区分「读过了」与「过期作废」，同时又不能永久阻塞
    /// Result 与 Goal 的保留清理。
    fn expire_results(
        &self,
        scope: &ScopeRef,
        result_ids: &[ResultId],
        expired_at_ms: u64,
    ) -> Result<u32, ResultStoreError>;
}
