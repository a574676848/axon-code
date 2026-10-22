//! 远控本地协议仿真 server。
//!
//! 该模块冻结 Feishu/mobile adapter 接入前的本地协议边界：请求必须携带
//! 稳定协议版本和可审计请求 ID，响应只返回 bounded projection 或本地投递报告。

use axon_domain::{
    AxonError, AxonResult, RemoteAction, RemoteCapabilityPolicy, RemoteSessionProjection,
    RuntimeFailure, SessionId,
};
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase, WireLifecycleStatus,
    WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
};
use serde::{Deserialize, Serialize};

use crate::{
    RemoteControlInboxRuntime, RemoteControlMessageInput, RemoteControlMessagePlan,
    RemoteProjectionRuntime,
};

/// 远控本地协议版本。
pub const REMOTE_PROTOCOL_VERSION: &str = "axon.remote.v1";
/// 远控协议 client id 最大字符数。
pub const REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS: usize = 96;
/// 远控协议 request id 最大字符数。
pub const REMOTE_PROTOCOL_REQUEST_ID_MAX_CHARS: usize = 128;
/// 远控协议投影文本最大字符数。
pub const REMOTE_PROTOCOL_TEXT_MAX_CHARS: usize = 240;
/// 远控通知单批最大条数。
pub const REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS: usize = 16;
/// 远控通知引用 ID 最大数量。
pub const REMOTE_PROTOCOL_NOTIFICATION_REF_LIMIT: usize = 6;
/// 远控协议摘要读取原因。
pub const REMOTE_PROTOCOL_REASON_SUMMARY_PROJECTED: &str = "remote_protocol:summary_projected";
/// 远控协议消息已规划原因。
pub const REMOTE_PROTOCOL_REASON_MESSAGE_PLANNED: &str = "remote_protocol:message_planned";
/// 远控协议通知本地投影原因。
pub const REMOTE_PROTOCOL_REASON_NOTIFICATION_PROJECTED: &str =
    "remote_protocol:notification_projected";
/// 远控协议通知订阅原因。
pub const REMOTE_PROTOCOL_REASON_NOTIFICATION_SUBSCRIBED: &str =
    "remote_protocol:notification_subscribed";
/// 远控协议通知批量投递原因。
pub const REMOTE_PROTOCOL_REASON_NOTIFICATION_BATCH_PROJECTED: &str =
    "remote_protocol:notification_batch_projected";

/// 远控通知 wire lifecycle part ID 前缀。
pub const REMOTE_NOTIFICATION_LIFECYCLE_PART_ID_PREFIX: &str = "lifecycle:remote-notification";

/// 远控本地协议请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProtocolRequest {
    /// 协议版本。
    pub protocol_version: String,
    /// 远控客户端 ID。
    pub client_id: String,
    /// 请求 ID。
    pub request_id: String,
    /// 请求载荷。
    pub payload: RemoteProtocolPayload,
}

/// 远控本地协议载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteProtocolPayload {
    /// 读取 bounded summary。
    ReadSummary {
        /// 目标会话。
        session_id: SessionId,
    },
    /// 投递远控消息。
    DeliverMessage {
        /// 远控消息。
        message: RemoteControlMessageInput,
    },
    /// 投影远控通知。
    SendNotification {
        /// 通知投影。
        notification: Box<RemoteNotificationProjection>,
    },
    /// 声明远控通知订阅。
    SubscribeNotifications {
        /// 订阅投影。
        subscription: RemoteNotificationSubscription,
    },
    /// 批量投递远控通知。
    DeliverNotifications {
        /// 订阅投影。
        subscription: RemoteNotificationSubscription,
        /// 通知列表。
        notifications: Vec<RemoteNotificationProjection>,
    },
}

/// 远控通知投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationProjection {
    /// 目标会话。
    pub session_id: SessionId,
    /// 通知类别。
    pub kind: RemoteNotificationKind,
    /// 通知对应的 wire 事件。
    pub wire_event: WireEventEnvelope,
    /// 通知标题。
    pub title: String,
    /// 有界正文。
    pub bounded_body: String,
    /// 关联引用 ID。
    pub reference_ids: Vec<String>,
    /// 本机查看建议。
    pub local_action_hint: String,
    /// 通知级别。
    pub severity: RemoteNotificationSeverity,
}

/// 远控通知类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteNotificationKind {
    /// 任务开始。
    TaskStarted,
    /// 测试完成。
    TestCompleted,
    /// 任务失败。
    TaskFailed,
    /// 需要用户输入。
    InputRequired,
    /// 线程完成。
    ThreadCompleted,
    /// 安全阻断。
    SafetyBlocked,
}

/// 远控通知级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteNotificationSeverity {
    /// 普通信息。
    Info,
    /// 用户直答：对用户请求的直接回复或结果投递。
    ///
    /// 它不是主动通知，必须立即出站：禁止套用主动通知的静默期延期、
    /// 合并与限流，否则用户提问会被延迟到静默期结束。
    DirectReply,
    /// 需要关注。
    Warning,
    /// 需要用户处理。
    ActionRequired,
}

/// 远控通知订阅。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationSubscription {
    /// 目标会话。
    pub session_id: SessionId,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 订阅的通知类别。
    pub kinds: Vec<RemoteNotificationKind>,
    /// 单批最大通知数量。
    pub max_batch: usize,
}

/// 远控通知订阅报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationSubscriptionReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 订阅的通知类别。
    pub subscribed_kinds: Vec<RemoteNotificationKind>,
    /// 单批最大通知数量。
    pub max_batch: usize,
    /// 是否只是本地协议投影。
    pub local_only: bool,
}

/// 远控通知批量投递报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationBatchReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 已投递通知。
    pub delivered: Vec<RemoteNotificationDelivery>,
    /// 因订阅或批量上限丢弃的数量。
    pub dropped_count: usize,
    /// 是否只是本地协议投影。
    pub local_only: bool,
}

/// 远控通知投递条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationDelivery {
    /// 通知类别。
    pub kind: RemoteNotificationKind,
    /// 通知对应的 wire 事件。
    pub wire_event: WireEventEnvelope,
    /// 有界正文。
    pub bounded_body: String,
    /// 关联引用 ID。
    pub reference_ids: Vec<String>,
    /// 本机查看建议。
    pub local_action_hint: String,
    /// 通知级别。
    pub severity: RemoteNotificationSeverity,
}

/// 构造远控通知生命周期 wire 事件。
#[must_use]
pub fn remote_notification_lifecycle_part_id(event_key: &str) -> String {
    format!("{REMOTE_NOTIFICATION_LIFECYCLE_PART_ID_PREFIX}:{event_key}")
}

/// 构造远控通知生命周期 wire 事件。
#[must_use]
pub fn remote_notification_wire_lifecycle_event(
    session_id: SessionId,
    event_key: &str,
    status: WireLifecycleStatus,
    title: &str,
    summary: Option<String>,
    recorded_at_ms: i64,
) -> WireEventEnvelope {
    remote_notification_wire_event(
        session_id,
        event_key,
        recorded_at_ms,
        WireEventKind::TurnLifecycle(WireTurnLifecycle {
            part_id: remote_notification_lifecycle_part_id(event_key),
            turn_id: event_key.to_owned(),
            phase: WireLifecyclePhase::RegularTurn,
            status,
            title: title.to_owned(),
            kind: Some("remote-notification".to_owned()),
            ui_phase: None,
            tool_name: None,
            attempt_index: None,
            summary,
            duration_ms: None,
            started_at_ms: None,
            total_tokens: None,
            cache_hit_rate_basis_points: None,
        }),
    )
}

/// 构造携带权威失败事实的远控通知 wire 事件。
#[must_use]
pub fn remote_notification_wire_runtime_error(
    session_id: SessionId,
    event_key: &str,
    failure: RuntimeFailure,
    recorded_at_ms: i64,
) -> WireEventEnvelope {
    remote_notification_wire_event(
        session_id,
        event_key,
        recorded_at_ms,
        WireEventKind::RuntimeError(failure),
    )
}

fn remote_notification_wire_event(
    session_id: SessionId,
    event_key: &str,
    recorded_at_ms: i64,
    kind: WireEventKind,
) -> WireEventEnvelope {
    WireEventEnvelope {
        schema_version: WireSchemaVersion::default(),
        event_id: WireCorrelationId(event_key.to_owned()),
        correlation_id: WireCorrelationId(event_key.to_owned()),
        session: WireSessionRef {
            session_id: session_id.0.to_string(),
            run_id: None,
            parent_session_id: None,
        },
        sequence: 1,
        recorded_at_ms,
        kind,
    }
}

/// 远控本地协议响应。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProtocolResponse {
    /// 协议版本。
    pub protocol_version: String,
    /// 远控客户端 ID。
    pub client_id: String,
    /// 请求 ID。
    pub request_id: String,
    /// 响应载荷。
    pub payload: RemoteProtocolResponsePayload,
    /// 是否允许网络发送。
    pub outbound_network_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控本地协议响应载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteProtocolResponsePayload {
    /// bounded summary。
    Summary(RemoteSessionProjection),
    /// 远控消息规划结果。
    MessagePlan(RemoteControlMessagePlan),
    /// 本地投递报告。
    DeliveryReport(RemoteProtocolDeliveryReport),
    /// 通知订阅报告。
    NotificationSubscription(RemoteNotificationSubscriptionReport),
    /// 通知批量投递报告。
    NotificationBatch(RemoteNotificationBatchReport),
}

/// 远控本地投递报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteProtocolDeliveryReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 投递通道。
    pub channel: RemoteProtocolChannel,
    /// 有界预览。
    pub bounded_preview: String,
    /// 是否只是本地仿真投影。
    pub local_only: bool,
}

/// 远控协议投递通道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteProtocolChannel {
    /// 通知。
    Notification,
}

/// 远控本地协议仿真 runtime。
#[derive(Debug, Clone)]
pub struct RemoteProtocolSimulatorRuntime {
    policy: RemoteCapabilityPolicy,
    projection: RemoteProjectionRuntime,
    inbox: RemoteControlInboxRuntime,
}

impl RemoteProtocolSimulatorRuntime {
    /// 创建远控本地协议仿真 runtime。
    #[must_use]
    pub fn new(policy: RemoteCapabilityPolicy) -> Self {
        Self {
            policy: policy.clone(),
            projection: RemoteProjectionRuntime::new(policy.clone()),
            inbox: RemoteControlInboxRuntime::new(policy),
        }
    }

    /// 处理远控本地协议请求。
    ///
    /// # Errors
    ///
    /// 当协议版本、client id、request id、投影文本无效，或远控策略拒绝
    /// 对应动作时返回错误。
    pub fn handle(&self, request: &RemoteProtocolRequest) -> AxonResult<RemoteProtocolResponse> {
        validate_protocol(request)?;

        match &request.payload {
            RemoteProtocolPayload::ReadSummary { session_id } => {
                let summary = self.projection.project_summary(*session_id)?;
                Ok(response(
                    request,
                    RemoteProtocolResponsePayload::Summary(summary),
                    vec![REMOTE_PROTOCOL_REASON_SUMMARY_PROJECTED.to_owned()],
                ))
            }
            RemoteProtocolPayload::DeliverMessage { message } => {
                let plan = self.inbox.plan(message)?;
                Ok(response(
                    request,
                    RemoteProtocolResponsePayload::MessagePlan(plan),
                    vec![REMOTE_PROTOCOL_REASON_MESSAGE_PLANNED.to_owned()],
                ))
            }
            RemoteProtocolPayload::SendNotification { notification } => {
                self.policy
                    .ensure_allowed(RemoteAction::ReadBoundedSummary)?;
                validate_notification(notification)?;
                Ok(response(
                    request,
                    RemoteProtocolResponsePayload::DeliveryReport(delivery_report(
                        notification.session_id,
                        RemoteProtocolChannel::Notification,
                        &notification.bounded_body,
                    )),
                    vec![REMOTE_PROTOCOL_REASON_NOTIFICATION_PROJECTED.to_owned()],
                ))
            }
            RemoteProtocolPayload::SubscribeNotifications { subscription } => {
                self.policy
                    .ensure_allowed(RemoteAction::ReadBoundedSummary)?;
                validate_subscription(subscription)?;
                Ok(response(
                    request,
                    RemoteProtocolResponsePayload::NotificationSubscription(subscription_report(
                        subscription,
                    )),
                    vec![REMOTE_PROTOCOL_REASON_NOTIFICATION_SUBSCRIBED.to_owned()],
                ))
            }
            RemoteProtocolPayload::DeliverNotifications {
                subscription,
                notifications,
            } => {
                self.policy
                    .ensure_allowed(RemoteAction::ReadBoundedSummary)?;
                validate_subscription(subscription)?;
                validate_notification_batch(notifications)?;
                Ok(response(
                    request,
                    RemoteProtocolResponsePayload::NotificationBatch(notification_batch_report(
                        subscription,
                        notifications,
                    )),
                    vec![REMOTE_PROTOCOL_REASON_NOTIFICATION_BATCH_PROJECTED.to_owned()],
                ))
            }
        }
    }
}

fn validate_protocol(request: &RemoteProtocolRequest) -> AxonResult<()> {
    if request.protocol_version != REMOTE_PROTOCOL_VERSION {
        return Err(AxonError::runtime_failure("远控协议版本不匹配"));
    }
    validate_id(&request.client_id, REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS)?;
    validate_id(&request.request_id, REMOTE_PROTOCOL_REQUEST_ID_MAX_CHARS)?;
    Ok(())
}

fn validate_id(value: &str, limit: usize) -> AxonResult<()> {
    let actual = value.chars().count();
    if value.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if actual > limit {
        return Err(AxonError::InputTooLong { limit, actual });
    }
    Ok(())
}

fn validate_bounded_text(value: &str) -> AxonResult<()> {
    validate_id(value, REMOTE_PROTOCOL_TEXT_MAX_CHARS)
}

fn validate_subscription(subscription: &RemoteNotificationSubscription) -> AxonResult<()> {
    validate_id(
        &subscription.subscriber_id,
        REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS,
    )?;
    if subscription.kinds.is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if subscription.max_batch == 0 {
        return Err(AxonError::runtime_failure("远控通知批量大小无效"));
    }
    if subscription.max_batch > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS {
        return Err(AxonError::InputTooLong {
            limit: REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
            actual: subscription.max_batch,
        });
    }
    Ok(())
}

fn validate_notification_batch(notifications: &[RemoteNotificationProjection]) -> AxonResult<()> {
    if notifications.is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if notifications.len() > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS {
        return Err(AxonError::InputTooLong {
            limit: REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
            actual: notifications.len(),
        });
    }
    for notification in notifications {
        validate_notification(notification)?;
    }
    Ok(())
}

fn validate_notification(notification: &RemoteNotificationProjection) -> AxonResult<()> {
    if notification.wire_event.session.session_id != notification.session_id.0.to_string() {
        return Err(AxonError::runtime_failure(
            "远控通知 wire event session 不匹配",
        ));
    }
    validate_bounded_text(&notification.title)?;
    validate_bounded_text(&notification.bounded_body)?;
    validate_bounded_text(&notification.local_action_hint)?;
    if notification.reference_ids.len() > REMOTE_PROTOCOL_NOTIFICATION_REF_LIMIT {
        return Err(AxonError::InputTooLong {
            limit: REMOTE_PROTOCOL_NOTIFICATION_REF_LIMIT,
            actual: notification.reference_ids.len(),
        });
    }
    for reference_id in &notification.reference_ids {
        validate_bounded_text(reference_id)?;
    }
    Ok(())
}

fn response(
    request: &RemoteProtocolRequest,
    payload: RemoteProtocolResponsePayload,
    reasons: Vec<String>,
) -> RemoteProtocolResponse {
    RemoteProtocolResponse {
        protocol_version: REMOTE_PROTOCOL_VERSION.to_owned(),
        client_id: request.client_id.clone(),
        request_id: request.request_id.clone(),
        payload,
        outbound_network_allowed: false,
        reasons,
    }
}

fn delivery_report(
    session_id: SessionId,
    channel: RemoteProtocolChannel,
    preview: &str,
) -> RemoteProtocolDeliveryReport {
    RemoteProtocolDeliveryReport {
        session_id,
        channel,
        bounded_preview: preview.to_owned(),
        local_only: true,
    }
}

fn subscription_report(
    subscription: &RemoteNotificationSubscription,
) -> RemoteNotificationSubscriptionReport {
    RemoteNotificationSubscriptionReport {
        session_id: subscription.session_id,
        subscriber_id: subscription.subscriber_id.clone(),
        subscribed_kinds: subscription.kinds.clone(),
        max_batch: subscription.max_batch,
        local_only: true,
    }
}

fn notification_batch_report(
    subscription: &RemoteNotificationSubscription,
    notifications: &[RemoteNotificationProjection],
) -> RemoteNotificationBatchReport {
    let candidates: Vec<_> = notifications
        .iter()
        .filter(|notification| {
            notification.session_id == subscription.session_id
                && subscription.kinds.contains(&notification.kind)
        })
        .collect();
    let delivered: Vec<_> = candidates
        .iter()
        .take(subscription.max_batch)
        .map(|notification| RemoteNotificationDelivery {
            kind: notification.kind,
            wire_event: notification.wire_event.clone(),
            bounded_body: notification.bounded_body.clone(),
            reference_ids: notification.reference_ids.clone(),
            local_action_hint: notification.local_action_hint.clone(),
            severity: notification.severity,
        })
        .collect();

    RemoteNotificationBatchReport {
        session_id: subscription.session_id,
        subscriber_id: subscription.subscriber_id.clone(),
        dropped_count: notifications.len().saturating_sub(delivered.len()),
        delivered,
        local_only: true,
    }
}
