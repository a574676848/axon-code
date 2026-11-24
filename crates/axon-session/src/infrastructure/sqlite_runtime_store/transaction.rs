//! SQLite 事务边界说明。
/// 事务一致性策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionBoundary {
    /// Action、checkpoint、event 和 continuation 必须同一事务提交。
    ActionCommit,
    /// lease 变更与 fencing 校验必须同一事务提交。
    LeaseClaim,
}
