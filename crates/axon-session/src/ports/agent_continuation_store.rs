//! 子代理终态 continuation 引用的持久化端口（合同 §5.3.3）。
//!
//! continuation 是「子代理干完了什么」的持久事实，供**下一轮主代理**（含重启恢复后的
//! 会话）装配进父上下文。它回答的问题与 mailbox 不同：
//!
//! - mailbox 问「这条协作消息送到了吗」；
//! - continuation 问「这个子代理的终态还值得让下一轮知道吗」。
//!
//! **引用必须重启后可解析**（合同硬性约束）：`artifact_path` 是磁盘绝对路径、
//! `summary` 是落盘文本，两者共同承担解析职责；`job_ref` 里的进程代际跨进程必然失效，
//! 因此它只作可读线索，任何解析路径都不得以它为主键或查询条件。
//!
//! **同一 continuation 最多产生一次有效执行**：产生侧靠幂等键唯一索引，
//! 消费侧靠条件 `UPDATE` 的受影响行数（[`AgentContinuationStore::consume_continuation`]）。
//! 这与 Work Ledger 无关：本表不写入 `jarvis_goals` / `jarvis_executions` / `jarvis_events`。

use std::fmt;

/// continuation 的生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentContinuationState {
    /// 尚未被任何一轮主代理装配。
    Pending,
    /// 已被某一轮主代理装配；这是「有效执行」的唯一标记。
    Consumed,
    /// 越过保留窗口，不再装配。
    Expired,
}

impl AgentContinuationState {
    /// 持久化取值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Consumed => "consumed",
            Self::Expired => "expired",
        }
    }

    /// 解析持久化取值；未知取值返回 `None`。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "consumed" => Some(Self::Consumed),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// 是否仍在等待装配。
    #[must_use]
    pub const fn is_pending(self) -> bool {
        matches!(self, Self::Pending)
    }
}

/// 子代理终态的一次 continuation 引用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentContinuationRecord {
    /// 稳定 continuation ID。
    pub continuation_id: String,
    /// 幂等键；唯一索引，同一子代理同一次执行的同一终态只落一行。
    pub idempotency_key: String,
    /// 根会话作用域。
    pub owner_id: String,
    /// 父会话 ID；恢复装配按它筛选。
    pub parent_session_id: String,
    /// 子代理会话 ID；跨进程稳定，是「同一子代理」的持久标识。
    pub child_session_id: String,
    /// 子代理在代理树中的规范路径。
    pub agent_path: String,
    /// 临时 Job 引用（`{runtime_epoch}:{job_id}`）；**只作可读线索**，不承担解析职责。
    pub job_ref: Option<String>,
    /// 子代理执行代际引用；用于审计与去重。
    pub execution_ref: Option<String>,
    /// 六类终态之一（`completed` / `refusal` / `failed` / `killed` / `timeout` / `unresponsive`）。
    pub terminal_kind: String,
    /// 有界退出原因。
    pub exit_reason: String,
    /// 终态摘要（落盘文本）；摘要级恢复的正文，有界。
    pub summary: String,
    /// 完整结果的磁盘绝对路径；摘要级恢复的解析凭据。
    pub artifact_path: Option<String>,
    /// 真实 token 用量：输入。
    pub token_input: Option<u64>,
    /// 真实 token 用量：输出。
    pub token_output: Option<u64>,
    /// 真实 token 用量：推理。
    pub token_reasoning: Option<u64>,
    /// 生命周期状态。
    pub state: AgentContinuationState,
    /// 被装配的时刻。
    pub consumed_at_ms: Option<u64>,
    /// 消费方标识（执行面 + 会话），用于审计。
    pub consumed_by: Option<String>,
    /// 尝试装配的次数。
    pub attempt_count: u32,
    /// 最近一次尝试时刻。
    pub last_attempt_at_ms: Option<u64>,
    /// 下次可尝试时刻。
    pub retry_at_ms: Option<u64>,
    /// 最近一次装配失败原因。
    pub last_error: Option<String>,
    /// 创建时刻。
    pub created_at_ms: u64,
    /// 失效时刻。
    pub expires_at_ms: u64,
}

impl AgentContinuationRecord {
    /// 按「待装配」语义构造记录。
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn pending(
        continuation_id: impl Into<String>,
        idempotency_key: impl Into<String>,
        owner_id: impl Into<String>,
        parent_session_id: impl Into<String>,
        child_session_id: impl Into<String>,
        agent_path: impl Into<String>,
        terminal_kind: impl Into<String>,
        exit_reason: impl Into<String>,
        summary: impl Into<String>,
        created_at_ms: u64,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            continuation_id: continuation_id.into(),
            idempotency_key: idempotency_key.into(),
            owner_id: owner_id.into(),
            parent_session_id: parent_session_id.into(),
            child_session_id: child_session_id.into(),
            agent_path: agent_path.into(),
            job_ref: None,
            execution_ref: None,
            terminal_kind: terminal_kind.into(),
            exit_reason: exit_reason.into(),
            summary: summary.into(),
            artifact_path: None,
            token_input: None,
            token_output: None,
            token_reasoning: None,
            state: AgentContinuationState::Pending,
            consumed_at_ms: None,
            consumed_by: None,
            attempt_count: 0,
            last_attempt_at_ms: None,
            retry_at_ms: None,
            last_error: None,
            created_at_ms,
            expires_at_ms,
        }
    }

    /// 绑定临时 Job 引用（只作可读线索）。
    #[must_use]
    pub fn with_job_ref(mut self, job_ref: impl Into<String>) -> Self {
        self.job_ref = Some(job_ref.into());
        self
    }

    /// 绑定执行代际引用。
    #[must_use]
    pub fn with_execution_ref(mut self, execution_ref: impl Into<String>) -> Self {
        self.execution_ref = Some(execution_ref.into());
        self
    }

    /// 绑定完整结果的磁盘路径。
    #[must_use]
    pub fn with_artifact_path(mut self, artifact_path: impl Into<String>) -> Self {
        self.artifact_path = Some(artifact_path.into());
        self
    }

    /// 绑定真实 token 用量。
    #[must_use]
    pub fn with_tokens(mut self, input: u64, output: u64, reasoning: u64) -> Self {
        self.token_input = Some(input);
        self.token_output = Some(output);
        self.token_reasoning = Some(reasoning);
        self
    }
}

/// continuation 查询过滤条件。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentContinuationQuery {
    /// 按根会话作用域过滤。
    pub owner_id: Option<String>,
    /// 按父会话过滤。
    pub parent_session_id: Option<String>,
    /// 按子代理会话过滤。
    pub child_session_id: Option<String>,
    /// 按生命周期状态过滤。
    pub state: Option<AgentContinuationState>,
    /// 最多返回条数；`0` 表示不返回任何记录。
    pub limit: u32,
}

/// 一次 continuation 写入的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentContinuationAppendOutcome {
    /// 首次写入。
    Inserted,
    /// 幂等键已绑定过一条记录；携带其当前状态。
    Duplicate(AgentContinuationState),
}

/// continuation 持久化错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentContinuationStoreError {
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for AgentContinuationStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(detail) => write!(formatter, "continuation 持久化失败：{detail}"),
        }
    }
}

impl std::error::Error for AgentContinuationStoreError {}

/// 子代理终态 continuation 的持久化端口。
pub trait AgentContinuationStore: Send + Sync {
    /// 幂等写入一条 continuation。
    fn append_continuation(
        &self,
        record: &AgentContinuationRecord,
    ) -> Result<AgentContinuationAppendOutcome, AgentContinuationStoreError>;

    /// 按 ID 读取；不存在时返回 `None`。
    fn continuation(
        &self,
        continuation_id: &str,
    ) -> Result<Option<AgentContinuationRecord>, AgentContinuationStoreError>;

    /// 按幂等键读取；不存在时返回 `None`。
    fn continuation_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<AgentContinuationRecord>, AgentContinuationStoreError>;

    /// 读取等待装配的 continuation，按创建顺序；用于摘要级恢复。
    ///
    /// 只返回未过期且已到重试时刻的记录。
    fn pending_continuations(
        &self,
        owner_id: &str,
        parent_session_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentContinuationRecord>, AgentContinuationStoreError>;

    /// 原子地把一条 continuation 标记为已装配。
    ///
    /// 返回 `true` 表示**本次**调用完成了 `pending -> consumed` 迁移，也就是
    /// 「这一次是有效执行」；并发或重复消费拿到 `false` 并必须放弃。合同 §8.2.2
    /// 「同一 continuation 最多产生一次有效执行」由本条件更新结构性保证。
    fn consume_continuation(
        &self,
        continuation_id: &str,
        consumed_by: &str,
        now_ms: u64,
    ) -> Result<bool, AgentContinuationStoreError>;

    /// 记录一次装配尝试失败并推进重试记账。
    fn record_continuation_failure(
        &self,
        continuation_id: &str,
        error: &str,
        now_ms: u64,
        retry_at_ms: u64,
    ) -> Result<bool, AgentContinuationStoreError>;

    /// 结算越过保留窗口的待装配记录；返回结算条数。
    fn expire_continuations(
        &self,
        owner_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<u32, AgentContinuationStoreError>;

    /// 按过滤条件列出 continuation；用于状态查询。
    fn list_continuations(
        &self,
        query: &AgentContinuationQuery,
    ) -> Result<Vec<AgentContinuationRecord>, AgentContinuationStoreError>;
}
