//! Session Runtime 持久化端口。

use crate::domain::runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord,
    SessionRuntimeRecord,
};
use axon_domain::{
    DaemonSessionId, InteractionId, RuntimeHostId, SessionActionId, SessionActionStatus,
    SessionRuntimeStatus,
};
use thiserror::Error;

use super::{
    AgentContinuationStore, AgentMailboxStore, ClientDeliveryStore, DecisionStore, ExecutionStore,
    GoalCommitStore, GoalMutationStore, GoalStore, HealthStore, InteractionQueryStore,
    InteractionStore, LedgerCommitStore, LedgerEventStore, ResultStore, WorkspaceStore,
};

/// Runtime store 错误。
#[derive(Debug, Error)]
pub enum RuntimeStoreError {
    /// 领域 ID 无效。
    #[error("运行时 ID 无效：{0}")]
    InvalidId(String),
    /// SQLite 或序列化错误。
    #[error("运行时持久化失败：{0}")]
    Storage(String),
    /// fencing 条件不满足。
    #[error("运行时 lease fencing 拒绝写入")]
    Fenced,
    /// 目标不存在。
    #[error("运行时记录不存在：{0}")]
    NotFound(String),
    /// 幂等冲突。
    #[error("运行时幂等键冲突：{0}")]
    Conflict(String),
}

/// Provider turn 提交 checkpoint 时的遥测。
#[derive(Debug, Clone, Default)]
pub struct CheckpointTelemetry {
    /// transcript/rollout 引用。
    pub transcript_ref: Option<String>,
    /// token 使用量。
    pub token_usage: Option<u64>,
    /// turn 耗时。
    pub time_usage_ms: Option<u64>,
    /// Agent tree 摘要。
    pub agent_tree_summary: Option<String>,
    /// tool receipt 序号。
    pub tool_receipt_sequence: Option<u64>,
}

/// 持久 Runtime Store 端口。
pub trait RuntimeStore:
    AgentMailboxStore
    + AgentContinuationStore
    + GoalStore
    + GoalMutationStore
    + GoalCommitStore
    + DecisionStore
    + ExecutionStore
    + InteractionStore
    + InteractionQueryStore
    + ClientDeliveryStore
    + ResultStore
    + LedgerEventStore
    + LedgerCommitStore
    + WorkspaceStore
    + HealthStore
    + Send
    + Sync
{
    /// 初始化 schema。
    fn initialize(&self) -> Result<(), RuntimeStoreError>;
    /// 保存完整 Goal 结果文档，返回可复查的绝对路径；落盘失败不得公布路径。
    fn save_goal_result_document(
        &self,
        session_id: &DaemonSessionId,
        execution_id: &axon_domain::ExecutionId,
        content: &str,
    ) -> Result<String, RuntimeStoreError>;
    /// 注册或接管 Host lease。
    fn claim_host(
        &self,
        host_id: &RuntimeHostId,
        pid: u32,
        process_start_id: Option<&str>,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<DaemonHostRecord, RuntimeStoreError>;
    /// 续租 Host。
    fn renew_host(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 创建升级 admission 并阻止旧 Host 接收新 Action。
    fn request_upgrade(
        &self,
        host: &DaemonHostRecord,
        handoff_token: &str,
        target_version: &str,
        deadline_at_ms: u64,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 将旧 Host 标记为可交接。
    fn mark_handoff_ready(
        &self,
        host: &DaemonHostRecord,
        handoff_token: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 到达升级 deadline 时中断旧 Host 正在执行的 Execution。
    ///
    /// 中断等价于进程中断：外部副作用结果未知，因此收敛为 `uncertain` 而不是盲目重跑。
    fn interrupt_host_actions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<u32, RuntimeStoreError>;
    /// 使用一次性 token 原子接管旧 Host 的运行态。
    fn claim_upgrade_replacement(
        &self,
        host_id: &RuntimeHostId,
        pid: u32,
        process_start_id: Option<&str>,
        handoff_token: &str,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<DaemonHostRecord, RuntimeStoreError>;
    /// 创建 Session Runtime。
    fn create_session(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 停止 Session 当前运行并取消所有未完成 Action。
    fn stop_session(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 原子停止旧 rollout 并创建新的 rollout 身份。
    fn reset_session(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 读取 Session Runtime。
    fn get_session(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 限量接管 owner lease 已失效的非终态 Session。
    fn recover_expired_sessions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError>;
    /// 更新 Session 状态并校验 fencing。
    fn set_session_status(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        status: SessionRuntimeStatus,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 入队 Action；dedupe key 命中时返回既有记录。
    fn enqueue_action(
        &self,
        action: &SessionActionRecord,
        now_ms: u64,
    ) -> Result<SessionActionRecord, RuntimeStoreError>;
    /// 领取一个 Action。
    fn claim_next_action(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<Option<SessionActionRecord>, RuntimeStoreError>;
    /// 续租当前 Host 正在执行的 Action 租约。
    ///
    /// 一次 provider 轮次可能长于租约时长：没有续租时，supervisor 的
    /// `quarantine_interrupted_executions` 会把仍在跑的 Execution 判成 `uncertain`
    /// 并中断它，产出一段没有逐字稿的空会话。执行期由工作线程按租约的一半周期
    /// 调用本方法，把 `lease_expires_at_ms` 持续推后。
    ///
    /// 只延长 `claimed_by` 与 `claimed_generation` 都匹配的当前 claim，且要求租约
    /// 尚未过期、没有被请求取消；任何一项不成立即返回 [`RuntimeStoreError::Fenced`]，
    /// 调用方必须停止续租，不得当成可重试错误。
    fn renew_action_lease(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 按索引读取 Session 的未完成 Action。
    fn list_pending_actions(
        &self,
        session_id: &DaemonSessionId,
        limit: u32,
    ) -> Result<Vec<SessionActionRecord>, RuntimeStoreError>;
    /// 取消仍在队列中的 Action。
    fn cancel_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        now_ms: u64,
    ) -> Result<SessionActionRecord, RuntimeStoreError>;
    /// 按状态索引读取需要运行的 Session，避免扫描全部 Session。
    fn list_runnable_execution_sessions(
        &self,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError>;
    /// 更新 Action 状态并校验 fencing。
    fn update_action_status(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        status: SessionActionStatus,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 原子记录执行失败；达到重试上限时进入死信，否则重新入队。
    fn fail_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        error: &str,
        max_attempts: u32,
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 有界隔离租约失效的 `running` Execution：收敛为 `uncertain` 并提交不确定 Result。
    ///
    /// 进程中断后无法证明外部副作用未发生，因此绝不自动重试。
    fn quarantine_interrupted_executions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 有界收敛租约失效的 `processing` Interaction：置为 interrupted 并追加 durable 终态。
    fn quarantine_interrupted_interactions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 原子提交当前 Execution、checkpoint、事件和显式下一次 Execution。
    fn commit_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        next_execution: Option<&SessionActionRecord>,
        settlement: Option<&super::ExecutionSettlement>,
        checkpoint: &CheckpointTelemetry,
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 追加事件并校验 Host generation。
    fn append_event(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        event_type: &str,
        payload_json: &str,
        now_ms: u64,
    ) -> Result<SessionEventRecord, RuntimeStoreError>;
    /// 在同一事务中按输入顺序追加一批事件并校验一次 Host generation。
    fn append_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 在同一事务中校验 Interaction processing lease 并按顺序追加事件。
    fn append_interaction_events(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        interaction_id: &InteractionId,
        lease_token: &str,
        events: &[(String, String)],
        now_ms: u64,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 读取 cursor 之后的事件。
    fn replay_events(
        &self,
        session_id: &DaemonSessionId,
        after_sequence: u64,
        limit: u32,
    ) -> Result<Vec<SessionEventRecord>, RuntimeStoreError>;
    /// 创建或更新持久调度计划。
    fn upsert_schedule(&self, schedule: &RuntimeScheduleRecord) -> Result<(), RuntimeStoreError>;
    /// 读取到期 schedule。
    fn claim_due_schedules(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        lease_duration_ms: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeScheduleRecord>, RuntimeStoreError>;
    /// 原子结算一个已 claim 的 schedule，并将下一次触发时间写回。
    fn settle_schedule(
        &self,
        schedule_id: &str,
        host: &DaemonHostRecord,
        next_run_at_ms: Option<u64>,
        now_ms: u64,
        failed: bool,
    ) -> Result<(), RuntimeStoreError>;
}
