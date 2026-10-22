//! agent 间消息 mailbox 的持久化端口。
//!
//! 产品定位：mailbox 是**跨轮协作事实**，与 Work Ledger（Goal/Execution）和临时 Job
//! 注册表（进程内）并列，但三者互不替代（合同 §4 不变量 8）。因此本端口只承载
//! 「消息是否送达目标代理」这一件事，不引用 Goal、不引用 Job 生命周期。
//!
//! 一条消息的接收方唯一（一个 `AgentPath`），因此「消息」与「投递记录」合并为一张表：
//! 投递进度由 [`AgentMailboxMessageRecord`] 的 `state` / `attempt_count` /
//! `last_attempt_at_ms` / `retry_at_ms` / `last_error` / `delivered_at_ms` 列组表达。
//! `jarvis_completion_*` 之所以拆成通知与渠道两层，是因为一条通知要投递给多个渠道；
//! mailbox 不适用该场景，拆表只会引入无意义的 1:1 连接。

use std::fmt;

/// 消息类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMailboxMessageKind {
    /// 协作消息：父代理或同级代理经 `send_message` 发出的指令。
    Collab,
    /// 完成消息：子代理终态向父代理 mailbox 的回流。
    Completion,
}

impl AgentMailboxMessageKind {
    /// 持久化取值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Collab => "collab",
            Self::Completion => "completion",
        }
    }

    /// 解析持久化取值；未知取值返回 `None`。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "collab" => Some(Self::Collab),
            "completion" => Some(Self::Completion),
            _ => None,
        }
    }
}

/// 消息的投递状态。
///
/// `Delivered` **不是终态**：投递确认只说消息进了目标 mailbox，读确认才说目标真的
/// 看到了它。两者分离是「已投递未读消息重启后可恢复」的前提（合同 §8.2.5）：
/// 若把 `Delivered` 当终态，重启后就再也找不回那些「已送达但没被读过」的消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMailboxMessageState {
    /// 尚未成功交付给目标代理；目标不可用时停留在此状态并累计尝试次数。
    Pending,
    /// 已成功交付给目标代理的 mailbox，但尚未被读取。
    Delivered,
    /// 已被目标代理读取（读确认）；这才是协作消息的正常终态。
    Read,
    /// 越过保留窗口或达到最大尝试次数，不再重试。
    Expired,
}

impl AgentMailboxMessageState {
    /// 持久化取值。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Delivered => "delivered",
            Self::Read => "read",
            Self::Expired => "expired",
        }
    }

    /// 解析持久化取值；未知取值返回 `None`。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "delivered" => Some(Self::Delivered),
            "read" => Some(Self::Read),
            "expired" => Some(Self::Expired),
            _ => None,
        }
    }

    /// 是否仍需继续投递。
    #[must_use]
    pub const fn is_deliverable(self) -> bool {
        matches!(self, Self::Pending)
    }

    /// 是否已经抵达终态。
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Read | Self::Expired)
    }
}

/// 一条持久化的 agent 间消息；投递进度由本记录的状态列组承载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMailboxMessageRecord {
    /// 稳定消息 ID；落盘时生成，对外只作标识，**不承担幂等职责**。
    pub message_id: String,
    /// 幂等键；唯一索引，同一键重复写入只保留首次写入的那条消息。
    ///
    /// 键必须跨进程稳定，因此不得包含进程内计数（合同风险 R-10）：
    /// 协作消息为 `collab:{owner}:{author}:{recipient}:{created_at_ms}`，
    /// 完成消息为 `completion:{owner}:{child_session_id}:{terminal}`。
    pub idempotency_key: String,
    /// 根会话作用域；`AgentPath` 只在根会话内有意义，跨会话同名路径必须隔离。
    pub owner_id: String,
    /// 发送方 `AgentPath`。
    pub author: String,
    /// 接收方 `AgentPath`。
    pub recipient: String,
    /// 消息类别。
    pub kind: AgentMailboxMessageKind,
    /// 消息正文；写入前收敛到有界长度。
    pub content: String,
    /// 是否请求触发目标 turn。
    pub trigger_turn: bool,
    /// 完成消息关联的 continuation 记录 ID；协作消息为 `None`。
    pub continuation_id: Option<String>,
    /// 投递状态。
    pub state: AgentMailboxMessageState,
    /// 已投递尝试次数。
    pub attempt_count: u32,
    /// 最近一次尝试时刻。
    pub last_attempt_at_ms: Option<u64>,
    /// 下次可重试时刻；`None` 表示立即可试。
    pub retry_at_ms: Option<u64>,
    /// 最近一次失败原因（有界）。
    pub last_error: Option<String>,
    /// 投递成功时刻。
    pub delivered_at_ms: Option<u64>,
    /// 入队时刻。
    pub created_at_ms: u64,
    /// 失效时刻；越过后不再投递。
    pub expires_at_ms: u64,
}

impl AgentMailboxMessageRecord {
    /// 按「首次入队、待投递」语义构造记录。
    #[must_use]
    pub fn pending(
        message_id: impl Into<String>,
        idempotency_key: impl Into<String>,
        owner_id: impl Into<String>,
        author: impl Into<String>,
        recipient: impl Into<String>,
        kind: AgentMailboxMessageKind,
        content: impl Into<String>,
        trigger_turn: bool,
        created_at_ms: u64,
        expires_at_ms: u64,
    ) -> Self {
        Self {
            message_id: message_id.into(),
            idempotency_key: idempotency_key.into(),
            owner_id: owner_id.into(),
            author: author.into(),
            recipient: recipient.into(),
            kind,
            content: content.into(),
            trigger_turn,
            continuation_id: None,
            state: AgentMailboxMessageState::Pending,
            attempt_count: 0,
            last_attempt_at_ms: None,
            retry_at_ms: None,
            last_error: None,
            delivered_at_ms: None,
            created_at_ms,
            expires_at_ms,
        }
    }

    /// 绑定完成消息关联的 continuation 记录。
    #[must_use]
    pub fn with_continuation(mut self, continuation_id: impl Into<String>) -> Self {
        self.continuation_id = Some(continuation_id.into());
        self
    }
}

/// mailbox 查询过滤条件。
///
/// 三个过滤维度都可省略；省略表示不按该维度限定。用于 `axon debug mailbox`
/// 与恢复装配筛选，两者共用同一查询口径以免观测与恢复看到不同的事实。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentMailboxQuery {
    /// 按根会话作用域过滤。
    pub owner_id: Option<String>,
    /// 按接收方 `AgentPath` 过滤。
    pub recipient: Option<String>,
    /// 按投递状态过滤。
    pub state: Option<AgentMailboxMessageState>,
    /// 最多返回条数；`0` 表示不返回任何记录。
    pub limit: u32,
}

/// 投递失败后的重试策略。
///
/// 策略数值由调用方给出（内核的 `runtime_limits` 是阈值 SSOT），存储层只负责按当前
/// 尝试次数算出下一次重试时刻：`min(backoff_ms × 2^(attempt-1), max_backoff_ms)`。
/// 把这段算术放在存储层，是因为只有它掌握 `attempt_count`，而且「判定是否达到上限」
/// 与「自增尝试次数」必须在同一个 IMMEDIATE 事务内完成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentMailboxRetryPolicy {
    /// 首次重试的退避窗口，毫秒。
    pub backoff_ms: u64,
    /// 指数退避的上限窗口，毫秒。
    pub max_backoff_ms: u64,
    /// 最大尝试次数；达到后消息转 `expired`，不再重试。
    pub max_attempts: u32,
}

impl AgentMailboxRetryPolicy {
    /// 按刚失败的尝试序号（从 1 开始）计算下一次重试时刻。
    #[must_use]
    pub fn retry_at_ms(&self, now_ms: u64, attempt: u64) -> u64 {
        let exponent = attempt.saturating_sub(1).min(32);
        let factor = 1_u64
            .checked_shl(u32::try_from(exponent).unwrap_or(32))
            .unwrap_or(u64::MAX);
        let window = self
            .backoff_ms
            .saturating_mul(factor)
            .min(self.max_backoff_ms);
        now_ms.saturating_add(window)
    }
}

/// 一次消息写入的结果。
///
/// 必须区分「首次写入」与「幂等命中」：前者调用方继续投递，后者说明该幂等键
/// 已经绑定过一条消息，**不得**再次投递（合同 §5.3.1「同幂等键重复投递只执行一次」）。
/// 幂等命中时一并回报既有记录的当前状态，使调用方能够如实回执而不是猜测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMailboxAppendOutcome {
    /// 首次写入。
    Inserted,
    /// 幂等键已绑定过一条消息；携带该消息的当前投递状态。
    Duplicate(AgentMailboxMessageState),
}

/// mailbox 持久化错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentMailboxStoreError {
    /// 持久化失败。
    Storage(String),
}

impl fmt::Display for AgentMailboxStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(detail) => write!(formatter, "mailbox 持久化失败：{detail}"),
        }
    }
}

impl std::error::Error for AgentMailboxStoreError {}

/// agent 间消息的持久化端口。
pub trait AgentMailboxStore: Send + Sync {
    /// 幂等写入一条消息；返回本次写入的结果裁决。
    ///
    /// 同一幂等键的重复写入不产生新消息（`ON CONFLICT DO NOTHING`），
    /// 因此「同幂等键重复投递只执行一次」是结构性保证而非调用方约定。
    fn append_agent_message(
        &self,
        message: &AgentMailboxMessageRecord,
    ) -> Result<AgentMailboxAppendOutcome, AgentMailboxStoreError>;

    /// 按消息 ID 读取；不存在时返回 `None`。
    fn agent_message(
        &self,
        message_id: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentMailboxStoreError>;

    /// 按幂等键读取；不存在时返回 `None`。
    fn agent_message_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<AgentMailboxMessageRecord>, AgentMailboxStoreError>;

    /// 读取某接收方下待投递的消息，按入队顺序；用于重启后的 mailbox 恢复。
    ///
    /// 只返回未过期且已到重试时刻的记录。
    fn pending_agent_messages(
        &self,
        owner_id: &str,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError>;

    /// 扫描指定作用域下已到重试时刻的待投递消息；用于有界重试。
    ///
    /// 按作用域过滤而不是全局扫描：重试是某个会话的装配动作，
    /// 它不该把别的会话的消息拉进自己的内存 mailbox。
    fn due_agent_messages(
        &self,
        owner_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError>;

    /// 标记投递成功；返回 `true` 表示本次确实完成 `pending -> delivered` 迁移。
    ///
    /// 条件更新保证同一消息只有一次有效迁移，并发或重复调用拿到 `false`。
    fn deliver_agent_message(
        &self,
        message_id: &str,
        now_ms: u64,
    ) -> Result<bool, AgentMailboxStoreError>;

    /// 读确认：返回 `true` 表示本次确实完成 `delivered -> read` 迁移。
    ///
    /// 读确认与投递确认分离：只有消息真正对模型可见时才允许推进到 `read`，
    /// 否则「已读」会变成一句没有依据的断言（合同 §8.2.5）。
    fn read_agent_message(
        &self,
        message_id: &str,
        now_ms: u64,
    ) -> Result<bool, AgentMailboxStoreError>;

    /// 读取某接收方「已投递未读」的消息，按投递顺序。
    ///
    /// 用于重启后的 mailbox 恢复：`Delivered` 不是终态，这些消息必须重新装回，
    /// 否则跨进程重挂的子代理会永远读不到上一进程已送达的协作消息。
    fn delivered_unread_agent_messages(
        &self,
        owner_id: &str,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError>;

    /// 记录一次投递失败并推进重试记账。
    ///
    /// 退避时刻由 [`AgentMailboxRetryPolicy`] 按当前尝试次数算出；达到最大尝试次数的
    /// 记录由本方法直接结算为 `expired`，返回 `true` 表示该消息已终止重试，
    /// 调用方无需自行折算尝试次数上限。
    fn record_agent_message_failure(
        &self,
        message_id: &str,
        error: &str,
        now_ms: u64,
        policy: AgentMailboxRetryPolicy,
    ) -> Result<bool, AgentMailboxStoreError>;

    /// 结算越过保留窗口的待投递消息；返回结算条数。
    fn expire_agent_messages(
        &self,
        owner_id: &str,
        now_ms: u64,
        limit: u32,
    ) -> Result<u32, AgentMailboxStoreError>;

    /// 按过滤条件列出消息；用于状态查询。
    fn list_agent_messages(
        &self,
        query: &AgentMailboxQuery,
    ) -> Result<Vec<AgentMailboxMessageRecord>, AgentMailboxStoreError>;
}
