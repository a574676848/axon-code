//! mailbox 持久化端口到共享 runtime 库的适配（合同 §5.3.1）。
//!
//! 内核（`axon-agent-runtime`）不依赖数据库，只持有
//! [`axon_agent_runtime::AgentMailboxPersistence`] 的 trait 对象；本模块把该端口
//! 接到 `axon-session` 的 `jarvis_agent_messages` 仓储上，并绑定当前会话作用域。
//!
//! 作用域绑定在本层而不是内核：`owner_id` 是「根会话」概念，只有装配层知道它，
//! 而幂等键需要它作为前缀才能既跨进程稳定又不跨会话误判。

use std::path::Path;
use std::sync::Arc;

use axon_agent_runtime::{
    AgentMailboxDeliveryState, AgentMailboxMessageKind, AgentMailboxPendingMessage,
    AgentMailboxPersistAck, AgentMailboxPersistRequest, AgentMailboxPersistence,
    AgentMailboxRetryPolicy,
};
use axon_domain::{AxonError, AxonResult};
use axon_session::{
    AgentMailboxAppendOutcome, AgentMailboxMessageKind as StoreMessageKind,
    AgentMailboxMessageRecord, AgentMailboxMessageState,
    AgentMailboxRetryPolicy as StoreRetryPolicy, AgentMailboxStore, SqliteRuntimeStore,
};

/// 共享 runtime 库上的 mailbox 持久化实现。
pub(crate) struct RuntimeMailboxPersistence {
    store: SqliteRuntimeStore,
    owner_id: String,
}

impl RuntimeMailboxPersistence {
    pub(crate) fn new(store: SqliteRuntimeStore, owner_id: impl Into<String>) -> Self {
        Self {
            store,
            owner_id: owner_id.into(),
        }
    }
}

impl AgentMailboxPersistence for RuntimeMailboxPersistence {
    fn persist_message(
        &self,
        request: &AgentMailboxPersistRequest,
    ) -> AxonResult<AgentMailboxPersistAck> {
        let record = AgentMailboxMessageRecord::pending(
            request.message_id.clone(),
            request.idempotency_key.clone(),
            self.owner_id.clone(),
            request.author.clone(),
            request.recipient.clone(),
            store_kind(request.kind),
            request.content.clone(),
            request.trigger_turn,
            request.created_at_ms,
            request.expires_at_ms,
        );
        match self
            .store
            .append_agent_message(&record)
            .map_err(mailbox_failure)?
        {
            AgentMailboxAppendOutcome::Inserted => Ok(AgentMailboxPersistAck::Inserted),
            AgentMailboxAppendOutcome::Duplicate(state) => {
                Ok(AgentMailboxPersistAck::Duplicate(delivery_state(state)))
            }
        }
    }

    fn pending_messages(
        &self,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> AxonResult<Vec<AgentMailboxPendingMessage>> {
        let records = self
            .store
            .pending_agent_messages(&self.owner_id, recipient, now_ms, limit)
            .map_err(mailbox_failure)?;
        Ok(records.into_iter().map(pending_view).collect())
    }

    fn due_messages(&self, now_ms: u64, limit: u32) -> AxonResult<Vec<AgentMailboxPendingMessage>> {
        let records = self
            .store
            .due_agent_messages(&self.owner_id, now_ms, limit)
            .map_err(mailbox_failure)?;
        Ok(records.into_iter().map(pending_view).collect())
    }

    fn mark_delivered(&self, message_id: &str, now_ms: u64) -> AxonResult<bool> {
        self.store
            .deliver_agent_message(message_id, now_ms)
            .map_err(mailbox_failure)
    }

    fn mark_read(&self, message_id: &str, now_ms: u64) -> AxonResult<bool> {
        self.store
            .read_agent_message(message_id, now_ms)
            .map_err(mailbox_failure)
    }

    fn delivered_unread_messages(
        &self,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> AxonResult<Vec<AgentMailboxPendingMessage>> {
        let records = self
            .store
            .delivered_unread_agent_messages(&self.owner_id, recipient, now_ms, limit)
            .map_err(mailbox_failure)?;
        Ok(records.into_iter().map(pending_view).collect())
    }

    fn record_delivery_failure(
        &self,
        message_id: &str,
        error: &str,
        now_ms: u64,
        policy: AgentMailboxRetryPolicy,
    ) -> AxonResult<()> {
        self.store
            .record_agent_message_failure(message_id, error, now_ms, store_policy(policy))
            .map(|_| ())
            .map_err(mailbox_failure)
    }

    fn expire_messages(&self, now_ms: u64, limit: u32) -> AxonResult<u32> {
        self.store
            .expire_agent_messages(&self.owner_id, now_ms, limit)
            .map_err(mailbox_failure)
    }
}

/// 构造绑定 `owner_id` 作用域的 mailbox 持久化实现。
///
/// 库路径与 `axon run` 主链路同源（`<axon_root>/data/runtime.sqlite3`），
/// 因此本机 run 与 daemon 共享同一份 mailbox 事实。
pub(crate) fn runtime_mailbox_persistence(
    config_path: &Path,
    owner_id: &str,
) -> Result<Arc<dyn AgentMailboxPersistence>, String> {
    Ok(Arc::new(RuntimeMailboxPersistence::new(
        super::run_agent_store::shared_runtime_store(config_path)?,
        owner_id,
    )))
}

const fn store_kind(kind: AgentMailboxMessageKind) -> StoreMessageKind {
    match kind {
        AgentMailboxMessageKind::Collab => StoreMessageKind::Collab,
        AgentMailboxMessageKind::Completion => StoreMessageKind::Completion,
    }
}

const fn runtime_kind(kind: StoreMessageKind) -> AgentMailboxMessageKind {
    match kind {
        StoreMessageKind::Collab => AgentMailboxMessageKind::Collab,
        StoreMessageKind::Completion => AgentMailboxMessageKind::Completion,
    }
}

const fn store_policy(policy: AgentMailboxRetryPolicy) -> StoreRetryPolicy {
    StoreRetryPolicy {
        backoff_ms: policy.backoff_ms,
        max_backoff_ms: policy.max_backoff_ms,
        max_attempts: policy.max_attempts,
    }
}

const fn delivery_state(state: AgentMailboxMessageState) -> AgentMailboxDeliveryState {
    match state {
        AgentMailboxMessageState::Pending => AgentMailboxDeliveryState::Pending,
        // 已读是投递成功之后的读确认，投递结果本身仍然是「已投递」。
        AgentMailboxMessageState::Delivered | AgentMailboxMessageState::Read => {
            AgentMailboxDeliveryState::Delivered
        }
        AgentMailboxMessageState::Expired => AgentMailboxDeliveryState::Expired,
    }
}

fn pending_view(record: AgentMailboxMessageRecord) -> AgentMailboxPendingMessage {
    AgentMailboxPendingMessage {
        message_id: record.message_id,
        author: record.author,
        recipient: record.recipient,
        kind: runtime_kind(record.kind),
        content: record.content,
        trigger_turn: record.trigger_turn,
        created_at_ms: record.created_at_ms,
    }
}

fn mailbox_failure(error: impl std::fmt::Display) -> AxonError {
    AxonError::runtime_failure(format!("mailbox 持久化失败：{error}"))
}
