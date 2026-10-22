//! mailbox 测试支撑：假持久化实现与用例夹具。
//!
//! 与用例分文件的理由与生产代码同源：支撑是「测试怎么搭台」，用例是「契约要求什么」
//! 两件事，混在一个文件里会让两边都难以单独演进，也会顶到单文件 700 行红线。

use std::sync::{Arc, Mutex};

use axon_domain::AxonResult;

use super::{
    AgentMailboxDeliveryState, AgentMailboxMessageKind, AgentMailboxPendingMessage,
    AgentMailboxPersistAck, AgentMailboxPersistRequest, AgentMailboxPersistence,
    AgentMailboxRetryPolicy, mailbox_expiry_at, millis_of,
};
use crate::agent_communication::AgentStatus;
use crate::agent_control_mailbox::{AgentMessageRequest, AgentMessageTrigger};
use crate::agent_control_runtime::{AgentControlRuntime, AgentPath, AgentSpawnRequest};
use crate::runtime_limits::{MAILBOX_DELIVERY_RETRY_BACKOFF, MAILBOX_DELIVERY_RETRY_MAX_BACKOFF};
pub(super) const NOW_MS: u64 = 1_000;

/// 内核实际下发的首次退避窗口；测试推进时间必须按它计算。
pub(super) fn production_first_backoff_ms() -> u64 {
    millis_of(MAILBOX_DELIVERY_RETRY_BACKOFF)
}

/// 内核实际下发的退避上限；测试用它一次跨过任何窗口。
pub(super) fn production_max_backoff_ms() -> u64 {
    millis_of(MAILBOX_DELIVERY_RETRY_MAX_BACKOFF)
}

/// 测试替身里的一条消息。
#[derive(Debug, Clone)]
pub(super) struct FakeMessage {
    request: AgentMailboxPersistRequest,
    delivered: bool,
    read: bool,
    expired: bool,
    attempt_count: u32,
    retry_at_ms: Option<u64>,
    last_error: Option<String>,
}

impl FakeMessage {
    pub(super) fn pending(request: AgentMailboxPersistRequest) -> Self {
        Self {
            request,
            delivered: false,
            read: false,
            expired: false,
            attempt_count: 0,
            retry_at_ms: None,
            last_error: None,
        }
    }

    pub(super) fn is_pending(&self) -> bool {
        !self.delivered && !self.expired
    }
}

#[derive(Default)]
pub(super) struct FakeMailboxPersistence {
    state: Mutex<Vec<FakeMessage>>,
}

impl FakeMailboxPersistence {
    pub(super) fn persisted(&self) -> Vec<AgentMailboxPersistRequest> {
        self.state
            .lock()
            .expect("锁")
            .iter()
            .map(|message| message.request.clone())
            .collect()
    }

    pub(super) fn failures(&self) -> Vec<(String, String)> {
        self.state
            .lock()
            .expect("锁")
            .iter()
            .filter_map(|message| {
                message
                    .last_error
                    .as_ref()
                    .map(|error| (message.request.message_id.clone(), error.clone()))
            })
            .collect()
    }

    pub(super) fn pending_count(&self) -> usize {
        self.state
            .lock()
            .expect("锁")
            .iter()
            .filter(|message| message.is_pending())
            .count()
    }

    pub(super) fn attempt_count(&self, message_id: &str) -> u32 {
        self.state
            .lock()
            .expect("锁")
            .iter()
            .find(|message| message.request.message_id == message_id)
            .map_or(0, |message| message.attempt_count)
    }

    /// 预置一条待投递消息，模拟上一进程留下的持久状态。
    pub(super) fn seed_pending(&self, request: AgentMailboxPersistRequest) {
        self.state
            .lock()
            .expect("锁")
            .push(FakeMessage::pending(request));
    }

    /// 预置一条「已投递未读」消息，模拟上一进程已送达但尚未被读取的状态。
    pub(super) fn seed_delivered(&self, request: AgentMailboxPersistRequest) {
        let mut message = FakeMessage::pending(request);
        message.delivered = true;
        self.state.lock().expect("锁").push(message);
    }

    /// 当前「已投递未读」的消息 ID。
    pub(super) fn delivered_unread(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("锁")
            .iter()
            .filter(|message| message.delivered && !message.read && !message.expired)
            .map(|message| message.request.message_id.clone())
            .collect()
    }
}

impl AgentMailboxPersistence for FakeMailboxPersistence {
    fn persist_message(
        &self,
        request: &AgentMailboxPersistRequest,
    ) -> AxonResult<AgentMailboxPersistAck> {
        let mut state = self.state.lock().expect("锁");
        if let Some(existing) = state
            .iter()
            .find(|message| message.request.idempotency_key == request.idempotency_key)
        {
            return Ok(AgentMailboxPersistAck::Duplicate(delivery_state(existing)));
        }
        state.push(FakeMessage::pending(request.clone()));
        Ok(AgentMailboxPersistAck::Inserted)
    }

    fn pending_messages(
        &self,
        recipient: &str,
        _now_ms: u64,
        limit: u32,
    ) -> AxonResult<Vec<AgentMailboxPendingMessage>> {
        let state = self.state.lock().expect("锁");
        Ok(state
            .iter()
            .filter(|message| message.is_pending() && message.request.recipient == recipient)
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(pending_view)
            .collect())
    }

    fn due_messages(&self, now_ms: u64, limit: u32) -> AxonResult<Vec<AgentMailboxPendingMessage>> {
        let state = self.state.lock().expect("锁");
        Ok(state
            .iter()
            .filter(|message| {
                message.is_pending()
                    && message.request.expires_at_ms > now_ms
                    && message
                        .retry_at_ms
                        .is_none_or(|retry_at| retry_at <= now_ms)
            })
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(pending_view)
            .collect())
    }

    fn mark_delivered(&self, message_id: &str, _now_ms: u64) -> AxonResult<bool> {
        let mut state = self.state.lock().expect("锁");
        for message in state.iter_mut() {
            if message.request.message_id == message_id && message.is_pending() {
                message.delivered = true;
                message.retry_at_ms = None;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn mark_read(&self, message_id: &str, _now_ms: u64) -> AxonResult<bool> {
        let mut state = self.state.lock().expect("锁");
        for message in state.iter_mut() {
            if message.request.message_id == message_id && message.delivered && !message.read {
                message.read = true;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn delivered_unread_messages(
        &self,
        recipient: &str,
        now_ms: u64,
        limit: u32,
    ) -> AxonResult<Vec<AgentMailboxPendingMessage>> {
        let state = self.state.lock().expect("锁");
        Ok(state
            .iter()
            .filter(|message| {
                message.delivered
                    && !message.read
                    && !message.expired
                    && message.request.recipient == recipient
                    && message.request.expires_at_ms > now_ms
            })
            .take(usize::try_from(limit).unwrap_or(usize::MAX))
            .map(pending_view)
            .collect())
    }

    fn record_delivery_failure(
        &self,
        message_id: &str,
        error: &str,
        now_ms: u64,
        policy: AgentMailboxRetryPolicy,
    ) -> AxonResult<()> {
        let mut state = self.state.lock().expect("锁");
        if let Some(message) = state
            .iter_mut()
            .find(|message| message.request.message_id == message_id && message.is_pending())
        {
            message.attempt_count = message.attempt_count.saturating_add(1);
            message.last_error = Some(error.to_owned());
            if u64::from(message.attempt_count) >= u64::from(policy.max_attempts) {
                message.expired = true;
                message.retry_at_ms = None;
            } else {
                message.retry_at_ms =
                    Some(now_ms.saturating_add(backoff_window(&policy, message.attempt_count)));
            }
        }
        Ok(())
    }

    fn expire_messages(&self, now_ms: u64, limit: u32) -> AxonResult<u32> {
        let mut state = self.state.lock().expect("锁");
        let mut expired = 0_u32;
        for message in state.iter_mut() {
            if expired >= limit {
                break;
            }
            if message.is_pending() && message.request.expires_at_ms <= now_ms {
                message.expired = true;
                expired = expired.saturating_add(1);
            }
        }
        Ok(expired)
    }
}

/// 与存储层同一公式的退避窗口，供测试替身复刻。
pub(super) fn backoff_window(policy: &AgentMailboxRetryPolicy, attempt: u32) -> u64 {
    let exponent = u64::from(attempt).saturating_sub(1).min(32);
    let factor = 1_u64
        .checked_shl(u32::try_from(exponent).unwrap_or(32))
        .unwrap_or(u64::MAX);
    policy
        .backoff_ms
        .saturating_mul(factor)
        .min(policy.max_backoff_ms)
}

pub(super) fn delivery_state(message: &FakeMessage) -> AgentMailboxDeliveryState {
    if message.delivered {
        AgentMailboxDeliveryState::Delivered
    } else if message.expired {
        AgentMailboxDeliveryState::Expired
    } else {
        AgentMailboxDeliveryState::Pending
    }
}

pub(super) fn pending_view(message: &FakeMessage) -> AgentMailboxPendingMessage {
    AgentMailboxPendingMessage {
        message_id: message.request.message_id.clone(),
        author: message.request.author.clone(),
        recipient: message.request.recipient.clone(),
        kind: message.request.kind,
        content: message.request.content.clone(),
        trigger_turn: message.request.trigger_turn,
        created_at_ms: message.request.created_at_ms,
    }
}

pub(super) fn spawn_request(task_name: &str) -> AgentSpawnRequest {
    AgentSpawnRequest {
        parent_path: AgentPath::root(),
        task_name: task_name.to_owned(),
        message: format!("处理 {task_name}"),
        nickname: None,
        role: None,
    }
}

pub(super) fn collab_request(
    author: AgentPath,
    recipient: AgentPath,
    content: &str,
) -> AgentMessageRequest {
    AgentMessageRequest {
        author,
        recipient,
        content: content.to_owned(),
        trigger: AgentMessageTrigger::QueueOnly,
        created_at_ms: NOW_MS,
        kind: AgentMailboxMessageKind::Collab,
    }
}

pub(super) fn attached_runtime(persistence: &Arc<FakeMailboxPersistence>) -> AgentControlRuntime {
    AgentControlRuntime::default()
        .with_mailbox_persistence("session-1", Arc::clone(persistence) as _)
}

pub(super) fn seed_request(message_id: &str, content: &str) -> AgentMailboxPersistRequest {
    AgentMailboxPersistRequest {
        message_id: message_id.to_owned(),
        idempotency_key: format!("collab:session-1:/root:/root/review:{NOW_MS}"),
        author: "/root".to_owned(),
        recipient: "/root/review".to_owned(),
        kind: AgentMailboxMessageKind::Collab,
        content: content.to_owned(),
        trigger_turn: false,
        created_at_ms: NOW_MS,
        // 与生产同源的失效时刻：重试循环的推进不得提前把消息判为过期。
        expires_at_ms: mailbox_expiry_at(NOW_MS),
    }
}

/// 让子代理进入终态，构造「合法但暂时不可接收」的目标。
pub(super) fn mark_recipient_unavailable(runtime: &mut AgentControlRuntime, path: &AgentPath) {
    runtime
        .update_status(path, AgentStatus::Completed(None))
        .expect("目标进入终态");
}

/// 让子代理关闭，构造「不可达」的目标（与「空闲」区分）。
pub(super) fn mark_recipient_closed(runtime: &mut AgentControlRuntime, path: &AgentPath) {
    runtime
        .update_status(path, AgentStatus::Shutdown)
        .expect("目标关闭");
}
