//! Daemon 运行时事件的临时流式桥接。

use axon_domain::{
    AxonError, AxonResult, ConsumerCursor, DaemonSessionId, InteractionId, InteractionStatus,
    RuntimeObservationFrame,
};
use axon_session::SessionEventRecord;
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
};
use std::time::Duration;

use super::event_stream_bridge_telemetry::{
    EventStreamBridgeTelemetry, EventStreamBridgeTelemetrySnapshot,
};

/// 单个实时订阅的 mailbox 槽位。
///
/// 回合收尾会在极短时间内连续提交 canonical 正文、usage、生命周期与 `turn_completed`；
/// 槽位不足会把健康的订阅判成 `subscriber_lag` 并强制重置重放，订阅方随后要靠 replay
/// 补齐。容量按「一次收尾突发的完整事件序列」留出余量，同时保持每个订阅者的内存上界可控
/// （槽位 × 单事件大小）。
const DEFAULT_SUBSCRIBER_CAPACITY: usize = 256;
const MAX_EVENT_STREAM_SUBSCRIBERS: usize = 1_024;
pub(crate) const STREAM_RESET_REASON_GENERATION_CHANGE: &str = "generation_change";
/// 发布线程乱序时回到 durable cursor 重放，避免实时投影收到倒序事件。
pub(crate) const STREAM_RESET_REASON_PUBLISH_ORDER: &str = "publish_order_violation";
/// 订阅者暂时落后时回到最后确认 cursor 重放。
pub(crate) const STREAM_RESET_REASON_SUBSCRIBER_LAG: &str = "subscriber_lag";

#[cfg(test)]
#[path = "event_stream_bridge_order_tests.rs"]
mod order_tests;
#[cfg(test)]
#[path = "event_stream_bridge_telemetry_tests.rs"]
mod telemetry_tests;
#[cfg(test)]
#[path = "event_stream_bridge_tests.rs"]
mod tests;

/// 一个已提交事件及其 durable cursor。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    /// 事件日志记录。
    pub event: SessionEventRecord,
    /// 事件提交后的 cursor。
    pub cursor: ConsumerCursor,
}

/// 一个实时订阅通知。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamNotification {
    /// 已提交的 Session 事件。
    SessionEvent(StreamEvent),
    /// 实时流被裁剪，客户端应携带 cursor 重连并按 durable replay 重建。
    StreamReset {
        /// 客户端最近确认的 durable cursor。
        cursor: Option<ConsumerCursor>,
        /// 裁剪原因。
        reason: &'static str,
    },
    /// Interaction 状态已完成 durable 提交。
    InteractionStatus(InteractionStatus),
    /// 运行时非持久观测帧（零落盘、可丢弃、不推进 durable cursor）。
    Observation(RuntimeObservationFrame),
}

#[derive(Debug)]
struct Subscriber {
    id: u64,
    /// 订阅者关心的 Interaction；`None` 表示**会话级订阅**（该会话的每条事件都投递）。
    ///
    /// 子代理会话不属于任何 Interaction，它的逐字稿没有 interaction 标注，
    /// 因此必须有一条不按 Interaction 过滤的订阅路径，界面才能像主会话一样实时接收。
    interaction_id: Option<InteractionId>,
    last_cursor: Option<ConsumerCursor>,
    sender: SyncSender<StreamNotification>,
    pending_reset: Arc<Mutex<Option<PendingStreamReset>>>,
}

#[derive(Debug, Clone)]
struct PendingStreamReset {
    cursor: Option<ConsumerCursor>,
    reason: &'static str,
}

impl Subscriber {
    fn request_reset(&self, reason: &'static str) {
        let mut pending = self
            .pending_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending.is_none() {
            *pending = Some(PendingStreamReset {
                cursor: self.last_cursor.clone(),
                reason,
            });
        }
    }

    fn reset_pending(&self) -> bool {
        self.pending_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some()
    }
}

#[derive(Debug, Default)]
struct SubscriberRegistry {
    by_session: HashMap<DaemonSessionId, Vec<Subscriber>>,
    total: usize,
}

/// 临时实时订阅桥接；不承担历史恢复或持久化职责。
#[derive(Debug, Clone)]
pub struct EventStreamBridge {
    subscribers: Arc<Mutex<SubscriberRegistry>>,
    next_subscriber_id: Arc<AtomicU64>,
    telemetry: EventStreamBridgeTelemetry,
}

impl EventStreamBridge {
    /// 创建桥接器。
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_telemetry(axon_session_runtime::first_token_telemetry_enabled())
    }

    fn new_with_telemetry(enabled: bool) -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(SubscriberRegistry::default())),
            next_subscriber_id: Arc::new(AtomicU64::new(0)),
            telemetry: EventStreamBridgeTelemetry::new(enabled),
        }
    }

    /// 订阅一个 Session 的新事件（按 Interaction 过滤）。
    ///
    /// `after_cursor` 仅用于过滤已经观察过的同代事件；历史事件必须通过
    /// `DaemonHost::attach` 从 durable store replay。
    pub fn subscribe(
        &self,
        session_id: DaemonSessionId,
        interaction_id: InteractionId,
        after_cursor: Option<ConsumerCursor>,
    ) -> AxonResult<EventStreamSubscription> {
        self.register_subscriber(session_id, Some(interaction_id), after_cursor)
    }

    /// 订阅一个 Session 的新事件（**会话级**：不按 Interaction 过滤）。
    ///
    /// 子代理会话用它：一条属于自己、不属于任何 Interaction 的会话流。
    pub fn subscribe_session(
        &self,
        session_id: DaemonSessionId,
        after_cursor: Option<ConsumerCursor>,
    ) -> AxonResult<EventStreamSubscription> {
        self.register_subscriber(session_id, None, after_cursor)
    }

    /// 使用指定 mailbox 容量订阅，容量为零时使用默认容量。
    pub fn subscribe_with_capacity(
        &self,
        session_id: DaemonSessionId,
        interaction_id: InteractionId,
        after_cursor: Option<ConsumerCursor>,
        capacity: usize,
    ) -> AxonResult<EventStreamSubscription> {
        self.register_subscriber_with_capacity(
            session_id,
            Some(interaction_id),
            after_cursor,
            capacity,
        )
    }

    /// 使用指定 mailbox 容量订阅一个 Session 的新事件（**会话级**：不按 Interaction 过滤）。
    pub fn subscribe_session_with_capacity(
        &self,
        session_id: DaemonSessionId,
        after_cursor: Option<ConsumerCursor>,
        capacity: usize,
    ) -> AxonResult<EventStreamSubscription> {
        self.register_subscriber_with_capacity(session_id, None, after_cursor, capacity)
    }

    fn register_subscriber(
        &self,
        session_id: DaemonSessionId,
        interaction_id: Option<InteractionId>,
        after_cursor: Option<ConsumerCursor>,
    ) -> AxonResult<EventStreamSubscription> {
        self.register_subscriber_with_capacity(
            session_id,
            interaction_id,
            after_cursor,
            DEFAULT_SUBSCRIBER_CAPACITY,
        )
    }

    fn register_subscriber_with_capacity(
        &self,
        session_id: DaemonSessionId,
        interaction_id: Option<InteractionId>,
        after_cursor: Option<ConsumerCursor>,
        capacity: usize,
    ) -> AxonResult<EventStreamSubscription> {
        let (sender, receiver) = mpsc::sync_channel(capacity.max(1));
        let mut registry = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if registry.total >= MAX_EVENT_STREAM_SUBSCRIBERS {
            return Err(AxonError::runtime_failure(
                "Daemon 实时事件订阅已达总量上限",
            ));
        }
        let id = self.next_subscriber_id.fetch_add(1, Ordering::Relaxed);
        let pending_reset = Arc::new(Mutex::new(None));
        registry
            .by_session
            .entry(session_id.clone())
            .or_default()
            .push(Subscriber {
                id,
                interaction_id,
                last_cursor: after_cursor,
                sender,
                pending_reset: Arc::clone(&pending_reset),
            });
        registry.total = registry.total.saturating_add(1);
        self.telemetry.record_subscriber_added();
        Ok(EventStreamSubscription {
            id,
            session_id,
            receiver,
            pending_reset,
            subscribers: Arc::downgrade(&self.subscribers),
            telemetry: self.telemetry.clone(),
        })
    }

    /// 向匹配 Session 的活跃订阅投递一个已提交事件。
    ///
    /// 慢消费者和断开连接只影响该消费者的临时投影，不会失败 Worker。
    /// 没有 Interaction 标注的事件（例如子代理会话）只会投给会话级订阅者。
    pub fn publish(&self, event: &SessionEventRecord, cursor: &ConsumerCursor) {
        let event_interaction_id = event_interaction_id(event);
        self.publish_matching(event_interaction_id.as_deref(), event, cursor);
    }

    /// 使用调用方已经确认的 InteractionId 发布事件，避免重复解析 payload_json。
    pub fn publish_for_interaction(
        &self,
        interaction_id: &InteractionId,
        event: &SessionEventRecord,
        cursor: &ConsumerCursor,
    ) {
        self.publish_matching(Some(interaction_id.as_str()), event, cursor);
    }

    fn publish_matching(
        &self,
        event_interaction_id: Option<&str>,
        event: &SessionEventRecord,
        cursor: &ConsumerCursor,
    ) {
        self.telemetry.record_publish();
        let lock_started_at = self.telemetry.lock_started_at();
        let mut registry = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.telemetry.record_lock_acquired(lock_started_at);
        let Some(subscribers) = registry.by_session.get_mut(&event.session_id) else {
            return;
        };
        let before = subscribers.len();
        subscribers.retain_mut(|subscriber| {
            // 会话级订阅者收该会话的每条事件；按 Interaction 订阅的只收自己那一条。
            let matches = match subscriber.interaction_id.as_ref() {
                None => true,
                Some(interaction_id) => event_interaction_id == Some(interaction_id.as_str()),
            };
            if !matches {
                return true;
            }
            self.telemetry.record_match();
            if subscriber.reset_pending() {
                return true;
            }
            if let Some(previous) = subscriber.last_cursor.as_ref() {
                if previous.generation != cursor.generation {
                    subscriber.request_reset(STREAM_RESET_REASON_GENERATION_CHANGE);
                    self.telemetry.record_generation_prune();
                    return true;
                }
                if cursor.sequence == previous.sequence {
                    // replay/live 交叠时同一 durable cursor 只做去重，不应触发乱序恢复。
                    return true;
                }
                if !cursor.advances_from(previous) {
                    subscriber.request_reset(STREAM_RESET_REASON_PUBLISH_ORDER);
                    self.telemetry.record_generation_prune();
                    return true;
                }
            }
            match subscriber
                .sender
                .try_send(StreamNotification::SessionEvent(StreamEvent {
                    event: event.clone(),
                    cursor: cursor.clone(),
                })) {
                Ok(()) => {
                    subscriber.last_cursor = Some(cursor.clone());
                    self.telemetry.record_delivered();
                    true
                }
                Err(TrySendError::Full(_)) => {
                    subscriber.request_reset(STREAM_RESET_REASON_SUBSCRIBER_LAG);
                    self.telemetry.record_mailbox_full_prune();
                    true
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.telemetry.record_disconnected_prune();
                    false
                }
            }
        });
        let removed = before.saturating_sub(subscribers.len());
        self.telemetry.record_subscribers_removed(removed);
        let session_is_empty = subscribers.is_empty();
        registry.total = registry.total.saturating_sub(removed);
        if session_is_empty {
            registry.by_session.remove(&event.session_id);
        }
    }

    /// 发布一个已经持久化的 Interaction 状态。
    pub fn publish_interaction_status(
        &self,
        session_id: &DaemonSessionId,
        interaction_id: &InteractionId,
        status: InteractionStatus,
    ) {
        let mut registry = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(subscribers) = registry.by_session.get_mut(session_id) else {
            return;
        };
        let before = subscribers.len();
        subscribers.retain(|subscriber| {
            // 交互状态只对盯着这个 Interaction 的订阅者有意义；会话级订阅者是
            // 「把这条会话的逐字稿照原样收下」，不该收到无关的交互状态帧。
            if subscriber.interaction_id.as_ref() != Some(interaction_id) {
                return true;
            }
            if subscriber.reset_pending() {
                return true;
            }
            match subscriber
                .sender
                .try_send(StreamNotification::InteractionStatus(status))
            {
                Ok(()) => {
                    self.telemetry.record_delivered();
                    true
                }
                Err(TrySendError::Full(_)) => {
                    subscriber.request_reset(STREAM_RESET_REASON_SUBSCRIBER_LAG);
                    self.telemetry.record_mailbox_full_prune();
                    true
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.telemetry.record_disconnected_prune();
                    false
                }
            }
        });
        let removed = before.saturating_sub(subscribers.len());
        self.telemetry.record_subscribers_removed(removed);
        registry.total = registry.total.saturating_sub(removed);
        if registry
            .by_session
            .get(session_id)
            .is_some_and(Vec::is_empty)
        {
            registry.by_session.remove(session_id);
        }
    }

    /// 发布一个非持久运行时观测帧给指定 Session 的订阅者。
    pub fn publish_observation(
        &self,
        session_id: &DaemonSessionId,
        frame: &RuntimeObservationFrame,
    ) {
        let mut registry = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(subscribers) = registry.by_session.get_mut(session_id) else {
            return;
        };
        let before = subscribers.len();
        subscribers.retain(|subscriber| {
            if subscriber.reset_pending() {
                return true;
            }
            match subscriber
                .sender
                .try_send(StreamNotification::Observation(frame.clone()))
            {
                Ok(()) => {
                    self.telemetry.record_delivered();
                    true
                }
                Err(TrySendError::Full(_)) => {
                    // 非持久观测帧允许丢弃，绝不触发 stream reset 或断开连接
                    true
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.telemetry.record_disconnected_prune();
                    false
                }
            }
        });
        let removed = before.saturating_sub(subscribers.len());
        self.telemetry.record_subscribers_removed(removed);
        registry.total = registry.total.saturating_sub(removed);
        if registry
            .by_session
            .get(session_id)
            .is_some_and(Vec::is_empty)
        {
            registry.by_session.remove(session_id);
        }
    }

    /// 广播一个非持久运行时观测帧给所有活跃会话订阅者。
    pub fn broadcast_observation(&self, frame: &RuntimeObservationFrame) {
        let mut registry = self
            .subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut total_removed = 0usize;
        let mut empty_sessions = Vec::new();
        for (session_id, subscribers) in registry.by_session.iter_mut() {
            let before = subscribers.len();
            subscribers.retain(|subscriber| {
                if subscriber.reset_pending() {
                    return true;
                }
                match subscriber
                    .sender
                    .try_send(StreamNotification::Observation(frame.clone()))
                {
                    Ok(()) => {
                        self.telemetry.record_delivered();
                        true
                    }
                    Err(TrySendError::Full(_)) => true,
                    Err(TrySendError::Disconnected(_)) => {
                        self.telemetry.record_disconnected_prune();
                        false
                    }
                }
            });
            let removed = before.saturating_sub(subscribers.len());
            total_removed = total_removed.saturating_add(removed);
            if subscribers.is_empty() {
                empty_sessions.push(session_id.clone());
            }
        }
        self.telemetry.record_subscribers_removed(total_removed);
        registry.total = registry.total.saturating_sub(total_removed);
        for session_id in empty_sessions {
            registry.by_session.remove(&session_id);
        }
    }

    /// 当前活跃订阅数量。
    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .total
    }

    /// 返回 trace-only Bridge 聚合指标。
    #[must_use]
    pub fn telemetry_snapshot(&self) -> EventStreamBridgeTelemetrySnapshot {
        self.telemetry.snapshot()
    }
}

impl Default for EventStreamBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// 订阅句柄；句柄释放时主动从桥接器注销。
pub struct EventStreamSubscription {
    id: u64,
    session_id: DaemonSessionId,
    receiver: Receiver<StreamNotification>,
    pending_reset: Arc<Mutex<Option<PendingStreamReset>>>,
    subscribers: Weak<Mutex<SubscriberRegistry>>,
    telemetry: EventStreamBridgeTelemetry,
}

impl EventStreamSubscription {
    /// 接收下一条实时通知。
    pub fn recv(&self) -> Result<StreamNotification, mpsc::RecvError> {
        match self.receiver.try_recv() {
            Ok(notification) => Ok(notification),
            Err(mpsc::TryRecvError::Empty) => self
                .take_pending_reset()
                .map_or_else(|| self.receiver.recv(), Ok),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.take_pending_reset().ok_or(mpsc::RecvError)
            }
        }
    }

    /// 在指定时限内等待下一条实时通知。
    pub fn recv_timeout(&self, timeout: Duration) -> Result<StreamNotification, RecvTimeoutError> {
        match self.receiver.try_recv() {
            Ok(notification) => return Ok(notification),
            Err(mpsc::TryRecvError::Disconnected) => {
                return self
                    .take_pending_reset()
                    .ok_or(RecvTimeoutError::Disconnected);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if let Some(reset) = self.take_pending_reset() {
            return Ok(reset);
        }
        match self.receiver.recv_timeout(timeout) {
            Err(RecvTimeoutError::Timeout) => {
                self.take_pending_reset().ok_or(RecvTimeoutError::Timeout)
            }
            result => result,
        }
    }

    /// 非阻塞接收实时通知。
    pub fn try_recv(&self) -> Result<StreamNotification, mpsc::TryRecvError> {
        match self.receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => {
                self.take_pending_reset().ok_or(mpsc::TryRecvError::Empty)
            }
            Err(mpsc::TryRecvError::Disconnected) => self
                .take_pending_reset()
                .ok_or(mpsc::TryRecvError::Disconnected),
            result => result,
        }
    }

    fn take_pending_reset(&self) -> Option<StreamNotification> {
        let notification = self
            .pending_reset
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .map(|reset| StreamNotification::StreamReset {
                cursor: reset.cursor,
                reason: reset.reason,
            });
        if notification.is_some() {
            self.unregister();
        }
        notification
    }

    fn unregister(&self) {
        let Some(subscribers) = self.subscribers.upgrade() else {
            return;
        };
        remove_subscription(&subscribers, &self.session_id, self.id, &self.telemetry);
    }
}

impl Drop for EventStreamSubscription {
    fn drop(&mut self) {
        self.unregister();
    }
}

fn remove_subscription(
    subscribers: &Mutex<SubscriberRegistry>,
    session_id: &DaemonSessionId,
    subscriber_id: u64,
    telemetry: &EventStreamBridgeTelemetry,
) {
    let mut registry = subscribers
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (removed, session_is_empty) = registry
        .by_session
        .get_mut(session_id)
        .map(|subscribers| {
            let before = subscribers.len();
            subscribers.retain(|subscriber| subscriber.id != subscriber_id);
            (subscribers.len() != before, subscribers.is_empty())
        })
        .unwrap_or((false, false));
    if removed {
        registry.total = registry.total.saturating_sub(1);
        telemetry.record_subscribers_removed(1);
    }
    if session_is_empty {
        registry.by_session.remove(session_id);
    }
}

fn event_interaction_id(event: &SessionEventRecord) -> Option<String> {
    let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json).ok()?;
    payload
        .get("interactionId")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}
