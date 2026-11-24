//! mailbox 持久化端口（合同 §5.3.1）。
//!
//! 内核不依赖数据库：`AgentControlRuntime` 只持有本端口的 trait 对象，由宿主
//! （`axon-cli` / daemon 装配层）注入 `axon-session` 的仓储实现。这与 M2 的
//! [`crate::SubagentJobInterrupt`] 同构：内核只调 trait，宿主提供实现。
//!
//! 未注入实现时，`AgentControlRuntime` 的 mailbox 行为与 M2 完全一致（纯内存）：
//! 落盘、幂等与重试都是**可选增强**，不是内核的固有职责。
//!
//! 幂等键必须跨进程稳定，因此不得包含进程内计数（合同风险 R-10）：
//! 协作消息为 `collab:{owner}:{author}:{recipient}:{created_at_ms}`，
//! 完成消息为 `completion:{owner}:{child_session_id}:{terminal}`。
//! `owner_id` 由宿主的实现绑定，内核不感知会话作用域。

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use axon_domain::AxonResult;
use uuid::Uuid;

use crate::agent_control_mailbox::{InterAgentCommunication, recipient_accepts_messages};
use crate::agent_control_runtime::{
    AGENT_CONTROL_REASON_RECIPIENT_UNAVAILABLE, AgentControlRuntime, AgentPath,
};
use crate::runtime_limits::{
    MAILBOX_DELIVERY_BATCH_LIMIT, MAILBOX_DELIVERY_MAX_ATTEMPTS, MAILBOX_DELIVERY_RETRY_BACKOFF,
    MAILBOX_DELIVERY_RETRY_MAX_BACKOFF, MAILBOX_HYDRATE_BATCH_LIMIT, MAILBOX_MESSAGE_MAX_CHARS,
    MAILBOX_MESSAGE_RETENTION,
};

/// 消息类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMailboxMessageKind {
    /// 协作消息：经 `send_message` 发出的指令。
    Collab,
    /// 完成消息：子代理终态向父代理 mailbox 的回流。
    Completion,
}

impl AgentMailboxMessageKind {
    /// 取值；幂等键的作用域前缀。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Collab => "collab",
            Self::Completion => "completion",
        }
    }
}

/// 一条待落盘的 mailbox 消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMailboxPersistRequest {
    /// 稳定消息 ID；由内核生成，供后续投递记账引用。
    pub message_id: String,
    /// 跨进程稳定的幂等键。
    pub idempotency_key: String,
    /// 发送方 `AgentPath`。
    pub author: String,
    /// 接收方 `AgentPath`。
    pub recipient: String,
    /// 消息类别。
    pub kind: AgentMailboxMessageKind,
    /// 已收敛到有界长度的消息正文。
    pub content: String,
    /// 是否请求触发目标 turn。
    pub trigger_turn: bool,
    /// 入队时刻。
    pub created_at_ms: u64,
    /// 失效时刻。
    pub expires_at_ms: u64,
}

/// 从持久表读回的待投递消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMailboxPendingMessage {
    /// 稳定消息 ID。
    pub message_id: String,
    /// 发送方 `AgentPath`。
    pub author: String,
    /// 接收方 `AgentPath`。
    pub recipient: String,
    /// 消息类别。
    pub kind: AgentMailboxMessageKind,
    /// 消息正文。
    pub content: String,
    /// 是否请求触发目标 turn。
    pub trigger_turn: bool,
    /// 入队时刻。
    pub created_at_ms: u64,
}

/// 一次投递的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMailboxDeliveryState {
    /// 消息已交付目标代理的 mailbox。
    Delivered,
    /// 目标代理暂时不可接收，消息已持久化并等待重试。
    Pending,
    /// 消息已越过保留窗口并被结算为失效，不再重试。
    Expired,
}

impl AgentMailboxDeliveryState {
    /// 渲染取值；用于工具回执的 `deliveryState` 字段。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Pending => "pending",
            Self::Expired => "expired",
        }
    }
}

/// 一次消息落盘的确认。
///
/// 幂等命中时必须回报既有记录的结局，否则调用方只能猜测这次重复投递的归宿，
/// 从而给出与实际不符的回执。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMailboxPersistAck {
    /// 首次写入。
    Inserted,
    /// 幂等键已绑定过一条消息；携带该消息的当前投递状态。
    Duplicate(AgentMailboxDeliveryState),
}

/// 投递失败后的重试策略。
///
/// 数值取自 `runtime_limits` 的阈值（SSOT 在内核），但**退避算术在存储层**：
/// 只有它掌握 `attempt_count`，而且「判定是否达到上限」与「自增尝试次数」
/// 必须在同一个 IMMEDIATE 事务内完成。
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
    /// 生产策略：从 `runtime_limits` 的阈值构造。
    #[must_use]
    pub fn from_runtime_limits() -> Self {
        Self {
            backoff_ms: millis_of(MAILBOX_DELIVERY_RETRY_BACKOFF),
            max_backoff_ms: millis_of(MAILBOX_DELIVERY_RETRY_MAX_BACKOFF),
            max_attempts: MAILBOX_DELIVERY_MAX_ATTEMPTS,
        }
    }
}

/// 一次有界重试的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MailboxDeliveryReport {
    /// 本次取出的到期待投递条数。
    pub retried: usize,
    /// 本次成功交付给目标 mailbox 的条数。
    pub delivered: usize,
    /// 目标仍不可接收、留在待投递状态的条数。
    pub still_pending: usize,
    /// 本次按保留窗口结算为失效的条数。
    pub expired: u32,
}

/// 把时长换算为毫秒；超出 `u64` 表示范围时饱和。
#[must_use]
pub fn millis_of(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// mailbox 持久化端口；由宿主实现。
pub trait AgentMailboxPersistence: Send + Sync {
    /// 幂等落盘一条消息。
    fn persist_message(
        &self,
        request: &AgentMailboxPersistRequest,
    ) -> AxonResult<AgentMailboxPersistAck>;

    /// 读取指定接收方下待投递的消息，按入队顺序。
    ///
    /// 用于重启后的 mailbox 恢复：新进程把仍待投递的消息重新装回内存 mailbox。
    fn pending_messages(
        &self,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> AxonResult<Vec<AgentMailboxPendingMessage>>;

    /// 读取本作用域下已到重试时刻的待投递消息；用于有界重试。
    ///
    /// 作用域由宿主在绑定时确定，因此内核不需要知道 `owner_id`。
    fn due_messages(&self, now_ms: u64, limit: u32) -> AxonResult<Vec<AgentMailboxPendingMessage>>;

    /// 标记投递成功；返回 `true` 表示本次确实完成迁移。
    fn mark_delivered(&self, message_id: &str, now_ms: u64) -> AxonResult<bool>;

    /// 读确认：返回 `true` 表示本次确实完成 `delivered -> read` 迁移。
    ///
    /// 只有消息真正作为模型可见内容被注入之后才允许调用；读确认与投递确认分离，
    /// 是「已投递未读消息重启后可恢复」的另一半（合同 §8.2.5）。
    fn mark_read(&self, message_id: &str, now_ms: u64) -> AxonResult<bool>;

    /// 读取指定接收方「已投递未读」的消息，按投递顺序。
    fn delivered_unread_messages(
        &self,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> AxonResult<Vec<AgentMailboxPendingMessage>>;

    /// 记录一次投递失败并推进重试记账。
    ///
    /// 退避时刻与「是否达到尝试上限」都由实现按 `policy` 与当前尝试次数裁决。
    fn record_delivery_failure(
        &self,
        message_id: &str,
        error: &str,
        now_ms: u64,
        policy: AgentMailboxRetryPolicy,
    ) -> AxonResult<()>;

    /// 结算越过保留窗口的待投递消息；返回结算条数。
    fn expire_messages(&self, now_ms: u64, limit: u32) -> AxonResult<u32>;
}

/// 内核持有的 mailbox 持久化槽位。
///
/// trait 对象不参与 `Debug` 派生，因此用本包装提供固定的占位输出，
/// 避免为了调试输出给端口增加与业务无关的约束。
#[derive(Clone, Default)]
pub(crate) struct MailboxPersistenceSlot(Option<Arc<dyn AgentMailboxPersistence>>);

impl MailboxPersistenceSlot {
    /// 绑定实现。
    pub(crate) fn attach(&mut self, persistence: Arc<dyn AgentMailboxPersistence>) {
        self.0 = Some(persistence);
    }

    /// 已绑定的实现；未绑定时为 `None`。
    pub(crate) fn get(&self) -> Option<&Arc<dyn AgentMailboxPersistence>> {
        self.0.as_ref()
    }
}

impl fmt::Debug for MailboxPersistenceSlot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(if self.0.is_some() {
            "MailboxPersistenceSlot(attached)"
        } else {
            "MailboxPersistenceSlot(detached)"
        })
    }
}

/// 把正文收敛到消息上限。
#[must_use]
pub fn bounded_mailbox_content(content: &str) -> String {
    content.chars().take(MAILBOX_MESSAGE_MAX_CHARS).collect()
}

/// 按消息保留窗口计算失效时刻。
#[must_use]
pub fn mailbox_expiry_at(created_at_ms: u64) -> u64 {
    let retention_ms = u64::try_from(MAILBOX_MESSAGE_RETENTION.as_millis()).unwrap_or(u64::MAX);
    created_at_ms.saturating_add(retention_ms)
}

/// 一次落盘尝试的结果。
///
/// 必须区分「未绑定持久化」与「幂等命中」：前者应继续正常投递，
/// 后者是重复投递，再次入队就等于同一消息执行了两次。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MailboxPersistOutcome {
    /// 未绑定持久化实现：按纯内存模式投递。
    Detached,
    /// 首次写入；携带本次分配的消息 ID 供后续记账。
    Persisted(String),
    /// 幂等键已绑定过一条消息：本次投递是重复的，不得再次入队；
    /// 携带既有记录的结局供调用方如实回执。
    Duplicate(AgentMailboxDeliveryState),
}

impl AgentControlRuntime {
    /// 绑定 mailbox 持久化实现与作用域。
    ///
    /// `owner_id` 是根会话作用域：同一 `AgentPath` 在不同根会话下必须隔离，
    /// 它同时是幂等键的作用域前缀，因此幂等键跨进程稳定且不会跨会话误判。
    #[must_use]
    pub fn with_mailbox_persistence(
        mut self,
        owner_id: impl Into<String>,
        persistence: Arc<dyn AgentMailboxPersistence>,
    ) -> Self {
        self.mailbox_owner = owner_id.into();
        self.mailbox_persistence.attach(persistence);
        self
    }

    /// 从另一个控制面继承 mailbox 持久化绑定与作用域。
    ///
    /// 拓扑恢复（`restore_open_tree_from_store`）会**新建**控制面实例；恢复出的实例
    /// 必须携带同一份绑定与同一个作用域前缀，否则跨轮 mailbox 事实会随实例替换而丢失，
    /// 幂等键也会因作用域清空而与先前写入的记录不一致。
    pub fn inherit_mailbox_persistence(&mut self, source: &Self) {
        self.mailbox_persistence = source.mailbox_persistence.clone();
        self.mailbox_owner.clone_from(&source.mailbox_owner);
    }

    /// 落盘一条 mailbox 消息，返回本次落盘的结果裁决。
    ///
    /// - [`MailboxPersistOutcome::Detached`]：未绑定持久化（纯内存模式）；
    /// - [`MailboxPersistOutcome::Persisted`]：首次写入，调用方应继续投递并用消息 ID 记账；
    /// - [`MailboxPersistOutcome::Duplicate`]：幂等键已绑定过一条消息，本次是重复投递，
    ///   **不得**再次进入内存 mailbox（合同 §5.3.1「同幂等键重复投递只执行一次」）。
    ///
    /// 落盘失败返回 `Err`：持久化失败必须可见，否则「消息不丢」无从保证。
    pub(crate) fn persist_mailbox_message(
        &mut self,
        author: &AgentPath,
        recipient: &AgentPath,
        kind: AgentMailboxMessageKind,
        content: &str,
        trigger_turn: bool,
        created_at_ms: u64,
    ) -> AxonResult<MailboxPersistOutcome> {
        let Some(persistence) = self.mailbox_persistence.get().cloned() else {
            return Ok(MailboxPersistOutcome::Detached);
        };
        let message_id = Uuid::new_v4().to_string();
        let request = AgentMailboxPersistRequest {
            message_id: message_id.clone(),
            idempotency_key: format!(
                "{}:{}:{author}:{recipient}:{created_at_ms}",
                kind.as_str(),
                self.mailbox_owner
            ),
            author: author.to_string(),
            recipient: recipient.to_string(),
            kind,
            content: content.to_owned(),
            trigger_turn,
            created_at_ms,
            expires_at_ms: mailbox_expiry_at(created_at_ms),
        };
        match persistence.persist_message(&request)? {
            AgentMailboxPersistAck::Inserted => Ok(MailboxPersistOutcome::Persisted(message_id)),
            AgentMailboxPersistAck::Duplicate(state) => Ok(MailboxPersistOutcome::Duplicate(state)),
        }
    }

    /// 把一条消息结算为已投递。
    pub(crate) fn mark_mailbox_delivered(&self, message_id: &str, now_ms: u64) {
        let Some(persistence) = self.mailbox_persistence.get() else {
            return;
        };
        // 记账失败的后果是重试时可能重复投递一次；调用方无法补救，因此只忽略结果，
        // 不把已成功的投递改判为失败。
        let _unused = persistence.mark_delivered(message_id, now_ms);
    }

    /// 记录一次投递失败；消息留在待投递状态由重试推进。
    ///
    /// 退避时刻与尝试上限由存储层按 `runtime_limits` 的策略裁决，内核不重复该规则。
    pub(crate) fn record_mailbox_delivery_failure(
        &self,
        message_id: &str,
        reason: &str,
        now_ms: u64,
    ) {
        let Some(persistence) = self.mailbox_persistence.get() else {
            return;
        };
        // 失败记账的结果只影响下次重试时刻；调用方无法补救，因此不把已发生的投递失败
        // 升级为调用错误。
        let _unused = persistence.record_delivery_failure(
            message_id,
            reason,
            now_ms,
            AgentMailboxRetryPolicy::from_runtime_limits(),
        );
    }

    /// 有界重试：把到期的待投递消息重新投递给目标，并结算越过保留窗口的消息。
    ///
    /// 重试复用**同一条持久记录与同一个幂等键**，因此重试不可能产生重复消息。
    /// 目标仍不可接收时按策略退避；达到尝试上限的记录由存储层结算为 `expired`。
    /// 每次调用只处理一个有界批次（`MAILBOX_DELIVERY_BATCH_LIMIT`），不无界拉取。
    ///
    /// 未绑定持久化时返回全零报告，行为与 M2 一致。
    pub fn drain_mailbox_delivery(&mut self, now_ms: u64) -> AxonResult<MailboxDeliveryReport> {
        let Some(persistence) = self.mailbox_persistence.get().cloned() else {
            return Ok(MailboxDeliveryReport::default());
        };
        let expired = persistence.expire_messages(now_ms, MAILBOX_DELIVERY_BATCH_LIMIT)?;
        let due = persistence.due_messages(now_ms, MAILBOX_DELIVERY_BATCH_LIMIT)?;
        let mut report = MailboxDeliveryReport {
            retried: due.len(),
            expired,
            ..MailboxDeliveryReport::default()
        };
        for message in due {
            let (Ok(recipient), Ok(author)) = (
                AgentPath::parse(&message.recipient),
                AgentPath::parse(&message.author),
            ) else {
                // 持久记录里的路径不可解析属数据损坏，不应阻断整批重试。
                report.still_pending = report.still_pending.saturating_add(1);
                continue;
            };
            if !recipient_accepts_messages(&self.status(&recipient)) {
                self.record_mailbox_delivery_failure(
                    &message.message_id,
                    AGENT_CONTROL_REASON_RECIPIENT_UNAVAILABLE,
                    now_ms,
                );
                report.still_pending = report.still_pending.saturating_add(1);
                continue;
            }
            self.mailboxes.ensure_agent(recipient.clone());
            self.mailboxes.push(
                recipient.clone(),
                InterAgentCommunication {
                    message_id: Some(message.message_id.clone()),
                    author,
                    recipient,
                    content: message.content,
                    trigger_turn: message.trigger_turn,
                    created_at_ms: message.created_at_ms,
                },
            );
            self.mark_mailbox_delivered(&message.message_id, now_ms);
            report.delivered = report.delivered.saturating_add(1);
        }
        Ok(report)
    }

    /// 从持久表把「待投递」与「已投递未读」消息装回内存 mailbox（合同 §5.3.1 / §8.2.5）。
    ///
    /// 在控制面完成拓扑恢复后调用：对每个已知代理读取两类消息，按入队顺序装回。
    /// 待投递的逐条结算为已投递；**已投递未读的原样装回、不改状态**：
    /// 它已经送达过，只是还没被读过，读确认要等真正消费时才推进。
    ///
    /// 未绑定持久化时返回 `0`，行为与 M2 一致。
    pub fn hydrate_mailbox(&mut self, now_ms: u64) -> AxonResult<usize> {
        // 装回是增量动作，先打标再执行：无论有没有持久化绑定，同一实例都只装回一次。
        self.mailboxes.mark_hydrated();
        let Some(persistence) = self.mailbox_persistence.get().cloned() else {
            return Ok(0);
        };
        let paths = self.agents.keys().cloned().collect::<Vec<_>>();
        let mut restored = 0_usize;
        for path in paths {
            // 两类都要**先读后写**：待投递消息在装回时会被结算为「已投递」，
            // 若先结算再查询，同一条消息会在「已投递未读」里被第二次读到，
            // 于是内存 mailbox 出现重复副本。
            let pending =
                persistence.pending_messages(path.as_str(), now_ms, MAILBOX_HYDRATE_BATCH_LIMIT)?;
            let unread = persistence.delivered_unread_messages(
                path.as_str(),
                now_ms,
                MAILBOX_HYDRATE_BATCH_LIMIT,
            )?;
            for message in pending {
                self.restore_mailbox_message(&path, &message)?;
                persistence.mark_delivered(&message.message_id, now_ms)?;
                restored = restored.saturating_add(1);
            }
            for message in unread {
                self.restore_mailbox_message(&path, &message)?;
                restored = restored.saturating_add(1);
            }
        }
        Ok(restored)
    }

    /// 一次性装回：已经装回过的实例直接返回 `0`。
    ///
    /// 装配点在**每次子代理工具调用**（`prepare_subagent_call`），因此必须自带
    /// 幂等保护：装回不是幂等操作，重复执行会把同一条持久消息推进内存队列多次。
    pub fn hydrate_mailbox_once(&mut self, now_ms: u64) -> AxonResult<usize> {
        if self.mailboxes.is_hydrated() {
            return Ok(0);
        }
        self.hydrate_mailbox(now_ms)
    }

    /// 取走某代理 mailbox 的全部待读消息，并把持久记录逐条结算为已读。
    ///
    /// **mailbox 的唯一消费出口**：内存队列被整体 drain，同一批消息不可能再从别处
    /// 取到，因此不会形成第二条并行注入路径（M3 设计 §7.5）。
    ///
    /// 读确认与取走同步发生：返回值已经持有这些消息，调用方要么注入它们，
    /// 要么让它们随返回值一起被丢弃：两种情况都不该再让它们停留在「未读」。
    pub fn take_mailbox_messages(
        &mut self,
        agent: &AgentPath,
        now_ms: u64,
    ) -> AxonResult<Vec<InterAgentCommunication>> {
        let messages = self.mailboxes.drain(agent);
        if messages.is_empty() {
            return Ok(messages);
        }
        if let Some(persistence) = self.mailbox_persistence.get().cloned() {
            for message in &messages {
                let Some(message_id) = message.message_id.as_deref() else {
                    continue;
                };
                persistence.mark_read(message_id, now_ms)?;
            }
        }
        Ok(messages)
    }

    /// 把一条持久消息装回内存 mailbox。
    fn restore_mailbox_message(
        &mut self,
        path: &AgentPath,
        message: &AgentMailboxPendingMessage,
    ) -> AxonResult<()> {
        let author = AgentPath::parse(&message.author)?;
        self.mailboxes.ensure_agent(path.clone());
        self.mailboxes.push(
            path.clone(),
            InterAgentCommunication {
                message_id: Some(message.message_id.clone()),
                author,
                recipient: path.clone(),
                content: message.content.clone(),
                trigger_turn: message.trigger_turn,
                created_at_ms: message.created_at_ms,
            },
        );
        Ok(())
    }
}

#[cfg(test)]
#[path = "agent_mailbox_test_support.rs"]
mod test_support;
#[cfg(test)]
#[path = "agent_mailbox_persistence_tests.rs"]
mod tests;
