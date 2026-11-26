//! Session Runtime 持久化端口。

use crate::domain::runtime::{
    DaemonHostRecord, RuntimeScheduleRecord, SessionActionRecord, SessionEventRecord, SessionKind,
    SessionRuntimeRecord, SessionTrustScope,
};
use axon_domain::{
    DaemonSessionId, RuntimeHostId, SessionActionId, SessionActionStatus, SessionRuntimeStatus,
};
use thiserror::Error;

/// 新建会话的显式身份属性。
///
/// 会话标识、类型、受众边界与归属都由调用方给出：服务端不推断会话，也不按投递目标派生。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSessionRequest {
    /// 会话标识，由调用方生成（ULID）。
    pub session_id: DaemonSessionId,
    /// 会话类型。
    pub kind: SessionKind,
    /// 受众隔离维度。
    pub trust_scope: SessionTrustScope,
    /// 归属 Workspace。
    pub workspace_id: Option<String>,
    /// 子代理的根会话；只有 `kind=Subagent` 才允许有值。
    pub root_session_id: Option<DaemonSessionId>,
    /// 子代理的代理路径；只有 `kind=Subagent` 才允许有值。
    ///
    /// 它与 `root_session_id` 一起构成子代理的身份：会话标识本身是 ULID，不携带路径，
    /// 因此「同一个父会话下的同一个代理」只能由这一对列认出来。
    pub agent_path: Option<String>,
    /// 目标执行会话的目标标识；只有 `kind=Execution` 才允许有值。
    pub goal_id: Option<String>,
    /// 目标执行会话的执行标识；只有 `kind=Execution` 才允许有值。
    pub execution_id: Option<String>,
    /// 工作目录。
    pub cwd: String,
    /// 创建时间。
    pub now_ms: u64,
}

impl NewSessionRequest {
    /// 主会话的缺省形态：私密受众、不绑定 Workspace 与目标。
    #[must_use]
    pub fn primary(session_id: DaemonSessionId, cwd: impl Into<String>, now_ms: u64) -> Self {
        Self {
            session_id,
            kind: SessionKind::Primary,
            trust_scope: SessionTrustScope::Private,
            workspace_id: None,
            root_session_id: None,
            agent_path: None,
            goal_id: None,
            execution_id: None,
            cwd: cwd.into(),
            now_ms,
        }
    }
}

/// 解析子代理会话的输入。
///
/// 子代理标识是 ULID，不能由父会话与路径拼出来，因此「同一个子代理」只能靠
/// `(root_session_id, agent_path)` 这一对认出来。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentSessionRequest {
    /// 根会话：派发这个子代理的那条会话。
    pub root_session_id: DaemonSessionId,
    /// 子代理在根会话内的代理路径。
    pub agent_path: String,
    /// 需要新建会话时使用的工作目录。
    pub cwd: String,
    /// 判定时间。
    pub now_ms: u64,
}

/// 解析目标执行会话的输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalSessionRequest {
    /// 目标标识。
    pub goal_id: String,
    /// 归属 Workspace；执行会话没有绑定时为空。
    pub workspace_id: Option<String>,
    /// 需要新建会话时使用的工作目录。
    pub cwd: String,
    /// 判定时间。
    pub now_ms: u64,
}

/// 解析定时任务的目标会话的输入。
///
/// 定时任务的会话绑定只在 `schedules.session_id` 一处：定义侧落定，唤醒侧只读。
/// 已有绑定就沿用，没有才建一条 `kind=cron` 会话，因此同一个任务的多轮唤醒落在同一条会话里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSessionRequest {
    /// 定时任务标识。
    pub schedule_id: String,
    /// 任务归属的 Workspace；没有绑定则为空。
    pub workspace_id: Option<String>,
    /// 判定时间。
    pub now_ms: u64,
}

use super::{
    AgentContinuationStore, AgentMailboxStore, ClientDeliveryStore, ContextSnapshotStore,
    DecisionStore, EventLogStore, ExecutionStore, GoalCommitStore, GoalMutationStore, GoalStore,
    HealthStore, InteractionQueryStore, InteractionSettlementStore, InteractionStore,
    LedgerCommitStore, LedgerEventStore, ResultStore, WorkspaceStore,
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
    /// 运行态数据与当前程序不兼容。
    ///
    /// 对外只给产品语义：版本号属于诊断细节，留在 `Debug` 与日志里。用户能做的动作只有
    /// 「重启后台服务」，把它翻译成 SQLite 的错误文案只会让人以为是自己操作错了。
    #[error("运行态数据与当前程序版本不一致，请重启 Axon 后台服务后重试")]
    DataIncompatible {
        /// 当前程序期望的 schema 版本。
        expected: i64,
        /// 库头实际记录的 schema 版本。
        found: i64,
    },
}

/// Provider turn 提交 checkpoint 时的遥测。
#[derive(Debug, Clone, Default)]
pub struct CheckpointTelemetry {
    /// transcript/运行会话引用。
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

/// 一次 `recovering` 收敛的结果。
///
/// `events` 是这次收敛产生的会话事件：控制面已经提交，事件面由调用方补齐并对外发布。
/// 它与 `quarantine_interrupted_interactions` 的返回值同一个用法。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveringSettlement {
    /// 收敛回 `idle` 的会话数（含两种来源：本来就没有在途工作项，与本次收口后腾空）。
    pub settled_to_idle: u32,
    /// 过期未派发而被收口为 `interrupted` 的工作项数。
    pub interrupted: u32,
    /// 需要追加到事件面的会话事件。
    pub events: Vec<SessionEventRecord>,
}

/// 持久 Runtime Store 端口。
///
/// 快照读取与在途轮次收口都是运行态事实的组成部分，因此作为超端口并入这里：
/// 任何完整的运行态存储都必须能回答「这条会话的上下文快照是什么」以及「这一轮还能不能
/// 收口」，而不是让调用方按具体实现再补一层类型参数。
pub trait RuntimeStore:
    AgentMailboxStore
    + AgentContinuationStore
    + ContextSnapshotStore
    + GoalStore
    + GoalMutationStore
    + GoalCommitStore
    + DecisionStore
    + ExecutionStore
    + InteractionStore
    + InteractionQueryStore
    + InteractionSettlementStore
    + ClientDeliveryStore
    + ResultStore
    + LedgerEventStore
    + LedgerCommitStore
    + WorkspaceStore
    + HealthStore
    + EventLogStore
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

    /// 把上一次进程留下的 `running` 执行重新排回队列，供 Daemon 启动时续跑。
    ///
    /// 与 [`Self::interrupt_host_actions`] 的方向相反：那条路径表达「外部结果未知，禁止
    /// 自动重跑」，这条只在**新进程刚启动、还没有任何自有执行**的瞬间使用，把旧进程的
    /// 在途执行交还调度器。必须在第一次 supervisor tick 之前调用，否则
    /// `quarantine_interrupted_executions` 会先把它们收敛成 `uncertain` 并落一条终态结果，
    /// 之后再重排就会撞结果主键。
    ///
    /// 只清租约、不改结果：旧执行从未提交过结果，重排后由新一轮 run 用 Thread 里已落库的
    /// 工具回执接着跑。改回 `queued` 后不再命中本条件，天然幂等。
    fn requeue_stale_running(&self, now_ms: u64) -> Result<u32, RuntimeStoreError>;
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
    ///
    /// 便捷入口：`kind='primary'`、`trust_scope='private'`、不绑定 Workspace 与目标。
    fn create_session(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 按显式身份属性创建 Session Runtime。
    ///
    /// 会话类型、受众边界与归属 Workspace 一律由调用方给出，服务端不推断；标识已存在时
    /// 返回既有行，不做原地改写。
    fn create_session_with(
        &self,
        request: &NewSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 解析某个根会话下某个代理路径的子代理会话；没有就建一条。
    ///
    /// 标识由服务端按 ULID 生成，先按 `(root_session_id, agent_path)` 查，查不到再建；
    /// 并发下撞唯一键时回读已有行，因此同一个子代理不会长出两条会话。
    fn resolve_subagent_session(
        &self,
        request: &SubagentSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 只读查某个根会话下某个代理路径的子代理会话；不存在返回 `None`，不创建。
    fn find_subagent_session(
        &self,
        root_session_id: &DaemonSessionId,
        agent_path: &str,
    ) -> Result<Option<SessionRuntimeRecord>, RuntimeStoreError>;
    /// 解析某个目标的执行会话；没有就建一条。
    fn resolve_goal_session(
        &self,
        request: &GoalSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 解析某个定时任务的目标会话；任务已绑定就沿用，没有绑定才新建。
    ///
    /// 标识由服务端按 ULID 生成，类型固定为 `cron`；只读不匹配的绑定不会被这里改写。
    fn resolve_schedule_session(
        &self,
        request: &ScheduleSessionRequest,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 按当前运行实例反查它所属的稳定会话标识。
    ///
    /// 运行实例标识只用于 rollout 恢复，不能当会话标识使用；需要会话身份的路径必须过这里。
    fn session_id_for_run(
        &self,
        current_run_id: &str,
    ) -> Result<Option<DaemonSessionId>, RuntimeStoreError>;
    /// 停止 Session 当前运行并取消所有未完成 Action。
    fn stop_session(
        &self,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        now_ms: u64,
    ) -> Result<SessionRuntimeRecord, RuntimeStoreError>;
    /// 原子停止旧运行会话并创建新的运行会话身份。
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
    /// 按标识批量读取会话行；不存在的标识不出现在结果里。
    ///
    /// 名册这类「一次要补 N 行的会话事实」按行回表会放大成 N 次点查；这里一次 IN 读回，
    /// 调用方按标识索引即可。
    fn sessions_by_ids(
        &self,
        session_ids: &[DaemonSessionId],
    ) -> Result<std::collections::HashMap<String, SessionRuntimeRecord>, RuntimeStoreError>;
    /// 按根会话批量读取它最近活动的子代理会话标识。
    ///
    /// 一次查询覆盖一整页，查询数与根会话条数无关。没有子会话的根不出现在结果里；
    /// 同一根有多条子会话时取最近活动的一条，因此调用方拿到的就是「这一轮落在哪条会话」。
    fn latest_subagent_session_ids(
        &self,
        root_session_ids: &[DaemonSessionId],
    ) -> Result<std::collections::HashMap<String, String>, RuntimeStoreError>;
    /// 该会话是否有正在运行且已被请求取消的执行轮次。
    ///
    /// 运行侧在后台执行期间按有界周期查询它：命中即中止本轮。取消与暂停目标都写这个标记，
    /// 这里是它唯一的消费点，因此「界面说停了、后台还在跑」不再成立。
    fn session_cancellation_requested(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<bool, RuntimeStoreError>;

    /// 请求取消目标会话当前正在跑的轮次，返回被标记的轮次数。
    ///
    /// 主 turn 由运行侧直接置位取消令牌，不需要这条路径；目标执行的取消只能由来源
    /// 会话调用 Goal 工具发起，工具成功后写一条取消标记，由运行侧的看门狗在有界
    /// 周期内消费。返回 0 表示这条会话当前没有在跑的轮次。
    fn request_session_cancellation(
        &self,
        session_id: &DaemonSessionId,
        reason: &str,
        now_ms: u64,
    ) -> Result<u64, RuntimeStoreError>;
    /// 推进子代理会话的状态与活跃时间。
    ///
    /// 子会话由派发方登记，但它是**被执行的那个会话**：状态必须反映真实运行态，
    /// 不能永远停在 `created`。这里只允许改 `kind='subagent'` 的行，不碰宿主租约。
    fn mark_subagent_session_status(
        &self,
        session_id: &DaemonSessionId,
        status: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 读该会话绑定的目标标识；没有绑定时为空。
    ///
    /// 子会话要继承根会话的目标归属，而 `SessionRuntimeRecord` 不带这一列，
    /// 因此单列读取，避免为一条归属事实去解析整行。
    fn session_goal_id(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<Option<String>, RuntimeStoreError>;
    /// 把会话当前轮次标识绑定到这一枚执行标识。
    ///
    /// 与会话行的其它轮次事实同一个定义：`execution_id` 就是「这一轮是谁」。
    /// 只对由 worker 驱动的会话（`goal` / `cron`）生效，主会话不使用这一列。
    fn bind_session_execution(
        &self,
        session_id: &DaemonSessionId,
        execution_id: &str,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 记录该会话最近一轮实际使用的 provider 与模型。
    ///
    /// 会话清单要显示 `provider · model`，这份事实只在轮次收口时产生；它属于会话元数据，
    /// 与是否产出指标无关。持久实现必须是单行点写且幂等：值没变时不产生写入。
    fn record_session_model(
        &self,
        session_id: &DaemonSessionId,
        provider: &str,
        model: &str,
        model_name: Option<&str>,
    ) -> Result<(), RuntimeStoreError> {
        let _ = (session_id, provider, model, model_name);
        Ok(())
    }
    /// 后台任务计数投影：`started` 为真表示作业开始，为假表示作业收口。
    ///
    /// 由作业生命周期事件的生产者调用；同一会话的计数只增不减到 0 为止，
    /// 因此重复投递同一条事件最多让 `jobs_running` 偏大，不会变成负数。
    fn apply_job_delta(
        &self,
        session_id: &DaemonSessionId,
        started: bool,
        failed: bool,
        now_ms: u64,
    ) -> Result<(), RuntimeStoreError>;
    /// 限量接管 owner lease 已失效的非终态 Session。
    fn recover_expired_sessions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<SessionRuntimeRecord>, RuntimeStoreError>;
    /// 收敛停在 `recovering` 的会话，保证这个状态一定有归宿（INV-7）。
    ///
    /// 两条收敛规则：
    /// 1. 没有在途工作项（`queued`/`running`）或它们已全部落定终态 → 会话回到 `idle`；
    /// 2. 停留在 `recovering` 超过 `ttl_ms`、工作项仍未派发 → 工作项收口为 `interrupted`
    ///    并追加对应事件，随后同样回到 `idle`。
    ///
    /// 运行中且租约仍然有效的工作项不动：那说明还有执行者，不属于「没有归宿」。
    fn settle_recovering_sessions(
        &self,
        host: &DaemonHostRecord,
        now_ms: u64,
        ttl_ms: u64,
        limit: u32,
    ) -> Result<RecoveringSettlement, RuntimeStoreError>;
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
    /// 原子提交当前 Execution、事件与显式下一次 Execution。
    ///
    /// 检查点不再单独落表：事实随本轮事件面帧一起落盘。
    fn commit_action(
        &self,
        action_id: &SessionActionId,
        session_id: &DaemonSessionId,
        host: &DaemonHostRecord,
        events: &[(String, String)],
        next_execution: Option<&SessionActionRecord>,
        settlement: Option<&super::ExecutionSettlement>,
        now_ms: u64,
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
