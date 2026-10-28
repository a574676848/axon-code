//! Work Ledger Execution 的持久化端口。

use axon_domain::{DaemonSessionId, ExecutionId, ExecutionRecord, ExecutionStatus};
use std::fmt;

/// Execution 存储错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStoreError {
    /// 记录不存在。
    NotFound(ExecutionId),
    /// 状态、lease 或幂等冲突。
    Conflict(String),
    /// 存储失败。
    Storage(String),
}

/// Scheduler 触发的显式 Execution 请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionTrigger {
    /// Execution 标识与幂等键。
    pub execution_id: ExecutionId,
    /// 关联的内部 Session。
    pub session_id: DaemonSessionId,
    /// 执行类型。
    pub kind: String,
    /// 优先级。
    pub priority: i32,
    /// 业务幂等键。
    pub dedupe_key: String,
    /// 执行 payload。
    pub payload_json: String,
}

impl fmt::Display for ExecutionStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(formatter, "Execution 不存在：{id}"),
            Self::Conflict(detail) => write!(formatter, "Execution 冲突：{detail}"),
            Self::Storage(detail) => write!(formatter, "Execution 存储失败：{detail}"),
        }
    }
}

impl std::error::Error for ExecutionStoreError {}

/// Execution 持久化端口。
pub trait ExecutionStore: Send + Sync {
    /// 精确读取 Goal 的历史幂等执行，不受列表分页和终态限制。
    fn execution_for_goal_dedupe(
        &self,
        goal_id: &axon_domain::GoalId,
        dedupe_key: &str,
    ) -> Result<Option<ExecutionRecord>, ExecutionStoreError>;
    /// 按 Goal 读取最近 Execution，结果按创建时间升序且有界。
    fn list_executions_for_goal(
        &self,
        _goal_id: &axon_domain::GoalId,
        _limit: u32,
    ) -> Result<Vec<ExecutionRecord>, ExecutionStoreError> {
        Ok(Vec::new())
    }
    /// 按运行时会话读取最近一条 Execution。
    ///
    /// 目标执行会话不靠标识前缀授权，而是靠这张账本里的 `session_id` 绑定：
    /// 网关据此判断「这个会话是不是某个目标执行派生出来的」，因此本端口是目标执行
    /// 会话订阅放行的唯一事实来源。
    fn execution_for_session(
        &self,
        _session_id: &DaemonSessionId,
    ) -> Result<Option<ExecutionRecord>, ExecutionStoreError> {
        Ok(None)
    }
    /// 幂等写入 Scheduler 触发的显式 Execution。
    fn enqueue_execution_trigger(
        &self,
        trigger: &ExecutionTrigger,
        now_ms: u64,
    ) -> Result<bool, ExecutionStoreError>;
    /// 幂等写入排队 Execution。
    fn enqueue_execution(&self, execution: &ExecutionRecord) -> Result<bool, ExecutionStoreError>;

    /// 读取 Execution。
    fn get_execution(
        &self,
        execution_id: &ExecutionId,
    ) -> Result<Option<ExecutionRecord>, ExecutionStoreError>;

    /// 领取可运行 Execution 并建立 lease。
    fn claim_executions(
        &self,
        limit: u32,
        owner: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Vec<ExecutionRecord>, ExecutionStoreError>;

    /// 延长当前 Execution lease。
    fn renew_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<(), ExecutionStoreError>;

    /// 记录可证明幂等的失败；未达上限时进入退避队列，达到上限时进入 failed 终态。
    fn fail_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        max_attempts: u32,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError>;

    /// 取消当前 Execution，并释放 lease。
    fn cancel_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError>;

    /// 以 lease token 更新状态并释放 lease。
    fn settle_execution(
        &self,
        execution_id: &ExecutionId,
        owner: &str,
        lease_token: &str,
        status: ExecutionStatus,
        now_ms: u64,
    ) -> Result<(), ExecutionStoreError>;
}
