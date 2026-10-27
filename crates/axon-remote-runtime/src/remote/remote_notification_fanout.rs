//! 远控通知 fanout 本地计划。
//!
//! 该模块把生命周期通知按已持久化订阅折叠为 adapter-neutral 投递计划；

use std::path::Path;

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS, REMOTE_PROTOCOL_NOTIFICATION_REF_LIMIT,
    REMOTE_PROTOCOL_TEXT_MAX_CHARS, RemoteNotificationDelivery, RemoteNotificationKind,
    RemoteNotificationProjection, RemoteSubscriptionStoreRecord, RemoteSubscriptionStoreRuntime,
    RemoteTransportKind,
};

/// 远控通知 fanout 原因：已读取订阅。
pub const REMOTE_NOTIFICATION_FANOUT_REASON_SUBSCRIPTIONS_LOADED: &str =
    "remote_notification_fanout:subscriptions_loaded";
/// 远控通知 fanout 原因：已筛选通知。
pub const REMOTE_NOTIFICATION_FANOUT_REASON_FILTERED: &str = "remote_notification_fanout:filtered";
/// 远控通知 fanout 原因：已生成 adapter 计划。
pub const REMOTE_NOTIFICATION_FANOUT_REASON_ADAPTER_PLANNED: &str =
    "remote_notification_fanout:adapter_planned";
/// 远控通知 fanout 原因：网络发送关闭。
pub const REMOTE_NOTIFICATION_FANOUT_REASON_NETWORK_CLOSED: &str =
    "remote_notification_fanout:network_closed";
/// 远控通知 fanout 原因：raw payload 关闭。
pub const REMOTE_NOTIFICATION_FANOUT_REASON_RAW_PAYLOAD_CLOSED: &str =
    "remote_notification_fanout:raw_payload_closed";

/// 单个订阅者的通知 fanout 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationFanoutPlan {
    /// 目标会话。
    pub session_id: SessionId,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 目标连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 订阅类别。
    pub subscribed_kinds: Vec<RemoteNotificationKind>,
    /// 已投递通知。
    pub delivered: Vec<RemoteNotificationDelivery>,
    /// 因订阅类别或 batch 上限丢弃的数量。
    pub dropped_count: usize,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
}

/// 远控通知 fanout 汇总报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationFanoutReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 输入通知数量。
    pub input_notification_count: usize,
    /// 活跃订阅数量。
    pub active_subscription_count: usize,
    /// 生成的投递计划。
    pub plans: Vec<RemoteNotificationFanoutPlan>,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控通知 fanout runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationFanoutRuntime;

impl RemoteNotificationFanoutRuntime {
    /// 从持久化订阅 store 生成通知 fanout 计划。
    ///
    /// # Errors
    ///
    /// 当通知为空、通知字段越界、store 读取失败或订阅记录无效时返回错误。
    pub fn fanout_from_store(
        path: impl AsRef<Path>,
        session_id: SessionId,
        notifications: &[RemoteNotificationProjection],
        now_ms: u64,
    ) -> AxonResult<RemoteNotificationFanoutReport> {
        validate_notifications(session_id, notifications)?;
        let subscriptions =
            RemoteSubscriptionStoreRuntime::active_for_session(path, session_id, now_ms)?;
        Ok(Self::fanout_from_records(
            session_id,
            notifications,
            &subscriptions,
        ))
    }

    fn fanout_from_records(
        session_id: SessionId,
        notifications: &[RemoteNotificationProjection],
        subscriptions: &[RemoteSubscriptionStoreRecord],
    ) -> RemoteNotificationFanoutReport {
        let plans = subscriptions
            .iter()
            .filter_map(|subscription| fanout_plan(subscription, notifications))
            .collect::<Vec<_>>();
        RemoteNotificationFanoutReport {
            session_id,
            input_notification_count: notifications.len(),
            active_subscription_count: subscriptions.len(),
            plans,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: vec![
                REMOTE_NOTIFICATION_FANOUT_REASON_SUBSCRIPTIONS_LOADED.to_owned(),
                REMOTE_NOTIFICATION_FANOUT_REASON_FILTERED.to_owned(),
                REMOTE_NOTIFICATION_FANOUT_REASON_ADAPTER_PLANNED.to_owned(),
                REMOTE_NOTIFICATION_FANOUT_REASON_NETWORK_CLOSED.to_owned(),
                REMOTE_NOTIFICATION_FANOUT_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
            ],
        }
    }
}

fn fanout_plan(
    subscription: &RemoteSubscriptionStoreRecord,
    notifications: &[RemoteNotificationProjection],
) -> Option<RemoteNotificationFanoutPlan> {
    let candidates = notifications
        .iter()
        .filter(|notification| subscription.kinds.contains(&notification.kind))
        .collect::<Vec<_>>();
    let delivered = candidates
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
        .collect::<Vec<_>>();
    if delivered.is_empty() {
        return None;
    }
    Some(RemoteNotificationFanoutPlan {
        session_id: SessionId(subscription.session_id),
        subscriber_id: subscription.subscriber_id.clone(),
        connection_id: subscription.connection_id.clone(),
        pairing_id: subscription.pairing_id.clone(),
        transport: subscription.transport,
        subscribed_kinds: subscription.kinds.clone(),
        dropped_count: notifications.len().saturating_sub(delivered.len()),
        delivered,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
    })
}

fn validate_notifications(
    session_id: SessionId,
    notifications: &[RemoteNotificationProjection],
) -> AxonResult<()> {
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
        if notification.session_id != session_id {
            return Err(AxonError::runtime_failure("远控通知 fanout session 不匹配"));
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
    }
    Ok(())
}

fn validate_bounded_text(value: &str) -> AxonResult<()> {
    let actual = value.chars().count();
    if value.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    if actual > REMOTE_PROTOCOL_TEXT_MAX_CHARS {
        return Err(AxonError::InputTooLong {
            limit: REMOTE_PROTOCOL_TEXT_MAX_CHARS,
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use axon_protocol::wire::{
        WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase,
        WireLifecycleStatus, WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
    };

    use crate::{
        REMOTE_SUBSCRIPTION_REASON_PERSISTED, RemoteNotificationSeverity,
        RemoteNotificationSubscription, RemoteSubscriptionStoreInput,
        remote_notification_lifecycle_part_id,
    };

    use super::*;

    #[test]
    fn notification_fanout_filters_by_subscription_and_batch_without_network() {
        let path = test_store_path("filters_by_subscription");
        let subscription =
            RemoteSubscriptionStoreRecord::from_input(RemoteSubscriptionStoreInput {
                subscription: RemoteNotificationSubscription {
                    session_id: SessionId(9),
                    subscriber_id: "mobile-client".to_owned(),
                    kinds: vec![RemoteNotificationKind::TaskStarted],
                    max_batch: 1,
                },
                connection_id: "mobile-conn".to_owned(),
                pairing_id: "pairing-local".to_owned(),
                transport: RemoteTransportKind::Mobile,
                recorded_at_ms: 100,
                updated_at_ms: 100,
                expires_at_ms: Some(1_000),
                revoked: false,
                source_trace: "remote_fanout:test".to_owned(),
                reason: REMOTE_SUBSCRIPTION_REASON_PERSISTED.to_owned(),
            })
            .expect("subscription record");

        RemoteSubscriptionStoreRuntime::append_record(&path, &subscription)
            .expect("append subscription");
        let report = RemoteNotificationFanoutRuntime::fanout_from_store(
            &path,
            SessionId(9),
            &[
                notification(RemoteNotificationKind::TaskStarted),
                notification(RemoteNotificationKind::TestCompleted),
            ],
            200,
        )
        .expect("fanout");

        assert_eq!(report.active_subscription_count, 1);
        assert_eq!(report.plans.len(), 1);
        assert_eq!(report.plans[0].delivered.len(), 1);
        assert_eq!(report.plans[0].dropped_count, 1);
        assert!(!report.network_send_allowed);
        assert!(!report.plans[0].network_send_allowed);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn notification_fanout_rejects_cross_session_notification() {
        let notification = RemoteNotificationProjection {
            session_id: SessionId(10),
            ..notification(RemoteNotificationKind::TaskStarted)
        };
        let result = RemoteNotificationFanoutRuntime::fanout_from_store(
            test_store_path("cross_session"),
            SessionId(9),
            &[notification],
            200,
        );
        assert!(result.is_err());
    }

    fn notification(kind: RemoteNotificationKind) -> RemoteNotificationProjection {
        RemoteNotificationProjection {
            session_id: SessionId(9),
            kind,
            wire_event: wire_lifecycle_event(SessionId(9), WireLifecycleStatus::Started),
            title: "远控生命周期通知".to_owned(),
            bounded_body: "通知正文只包含 bounded summary 与引用".to_owned(),
            reference_ids: vec!["thread:9".to_owned()],
            local_action_hint: "在本机查看线程详情".to_owned(),
            severity: RemoteNotificationSeverity::Info,
        }
    }

    fn wire_lifecycle_event(
        session_id: SessionId,
        status: WireLifecycleStatus,
    ) -> WireEventEnvelope {
        let event_key = format!("remote-notification-{}", session_id.0);
        WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(event_key.clone()),
            correlation_id: WireCorrelationId(event_key.clone()),
            session: WireSessionRef {
                session_id: session_id.0.to_string(),
                run_id: None,
                parent_session_id: None,
            },
            sequence: 1,
            recorded_at_ms: 100,
            kind: WireEventKind::TurnLifecycle(WireTurnLifecycle {
                part_id: remote_notification_lifecycle_part_id(&event_key),
                turn_id: event_key,
                phase: WireLifecyclePhase::RegularTurn,
                status,
                title: "远控生命周期通知".to_owned(),
                kind: Some("remote-notification".to_owned()),
                ui_phase: None,
                tool_name: None,
                attempt_index: None,
                summary: None,
                duration_ms: None,
                started_at_ms: None,
                total_tokens: None,
                cache_hit_rate_basis_points: None,

                warnings: Vec::new(),
            }),
        }
    }

    fn test_store_path(name: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon_remote_notification_fanout_{name}_{}.jsonl",
            std::process::id()
        ))
    }
}
