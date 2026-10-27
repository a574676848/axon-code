use axon_domain::{
    AxonError, RemoteAction, RemoteCapabilityPolicy, RemoteDenialReason, RemoteVisibility,
    SessionId,
};
use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireLifecyclePhase, WireLifecycleStatus,
    WireSchemaVersion, WireSessionRef, WireTurnLifecycle,
};

use super::{
    REMOTE_CONTROL_REASON_CONTROL_BYPASS, REMOTE_PROTOCOL_REASON_NOTIFICATION_BATCH_PROJECTED,
    REMOTE_PROTOCOL_REASON_NOTIFICATION_PROJECTED, REMOTE_PROTOCOL_REASON_NOTIFICATION_SUBSCRIBED,
    REMOTE_PROTOCOL_REASON_SUMMARY_PROJECTED, REMOTE_PROTOCOL_VERSION, RemoteControlCommand,
    RemoteControlDispatchAction, RemoteControlMessageInput, RemoteNotificationKind,
    RemoteNotificationProjection, RemoteNotificationSeverity, RemoteNotificationSubscription,
    RemoteProtocolChannel, RemoteProtocolPayload, RemoteProtocolRequest,
    RemoteProtocolResponsePayload, RemoteProtocolSimulatorRuntime,
    remote_notification_lifecycle_part_id,
};

#[test]
fn remote_protocol_projects_bounded_summary_without_network_or_stdin() {
    let response = runtime().handle(&summary_request()).unwrap();

    assert!(!response.outbound_network_allowed);
    assert!(
        response
            .reasons
            .contains(&REMOTE_PROTOCOL_REASON_SUMMARY_PROJECTED.to_owned())
    );
    assert_eq!(
        response.payload,
        RemoteProtocolResponsePayload::Summary(axon_domain::RemoteSessionProjection {
            session_id: SessionId(11),
            visibility: RemoteVisibility::BoundedSummary,
            truncated: false,
        })
    );
}

#[test]
fn remote_protocol_dispatches_control_message_through_inbox_guard() {
    let mut request = base_request();
    request.payload = RemoteProtocolPayload::DeliverMessage {
        message: RemoteControlMessageInput {
            delivery_id: "delivery-status".to_owned(),
            session_id: SessionId(11),
            raw_text: "/status".to_owned(),
            session_busy: true,
            seen_delivery_ids: Vec::new(),
        },
    };

    let response = runtime().handle(&request).unwrap();

    let RemoteProtocolResponsePayload::MessagePlan(plan) = response.payload else {
        panic!("应返回远控消息规划结果");
    };
    assert_eq!(plan.action, RemoteControlDispatchAction::InlineControl);
    assert_eq!(plan.command, Some(RemoteControlCommand::Status));
    assert!(
        plan.reasons
            .contains(&REMOTE_CONTROL_REASON_CONTROL_BYPASS.to_owned())
    );
}

#[test]
fn remote_protocol_projects_notification_locally() {
    let notification = runtime().handle(&notification_request()).unwrap();

    assert!(
        notification
            .reasons
            .contains(&REMOTE_PROTOCOL_REASON_NOTIFICATION_PROJECTED.to_owned())
    );
    assert_report(&notification.payload, RemoteProtocolChannel::Notification);
}

#[test]
fn remote_protocol_subscribes_and_delivers_bounded_lifecycle_notifications() {
    let subscription = runtime().handle(&subscription_request()).unwrap();
    let batch = runtime().handle(&notification_batch_request()).unwrap();

    assert!(!subscription.outbound_network_allowed);
    assert!(
        subscription
            .reasons
            .contains(&REMOTE_PROTOCOL_REASON_NOTIFICATION_SUBSCRIBED.to_owned())
    );
    let RemoteProtocolResponsePayload::NotificationSubscription(report) = subscription.payload
    else {
        panic!("应返回通知订阅报告");
    };
    assert_eq!(report.subscriber_id, "mobile-local-sim");
    assert!(report.local_only);
    assert_eq!(report.subscribed_kinds.len(), 6);

    assert!(!batch.outbound_network_allowed);
    assert!(
        batch
            .reasons
            .contains(&REMOTE_PROTOCOL_REASON_NOTIFICATION_BATCH_PROJECTED.to_owned())
    );
    let RemoteProtocolResponsePayload::NotificationBatch(report) = batch.payload else {
        panic!("应返回通知批量投递报告");
    };
    assert_eq!(report.delivered.len(), 6);
    assert_eq!(report.dropped_count, 1);
    assert!(report.local_only);
    assert!(
        report
            .delivered
            .iter()
            .any(|item| item.kind == RemoteNotificationKind::SafetyBlocked
                && wire_lifecycle_status(&item.wire_event) == Some(WireLifecycleStatus::Blocked)
                && item.reference_ids == vec!["policy:network-denied"])
    );
    assert!(
        report
            .delivered
            .iter()
            .all(|item| !item.bounded_body.is_empty() && !item.local_action_hint.is_empty())
    );
    let delivered_part_ids = report
        .delivered
        .iter()
        .filter_map(|item| match &item.wire_event.kind {
            WireEventKind::TurnLifecycle(lifecycle) => Some(lifecycle.part_id.as_str()),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(delivered_part_ids.len(), report.delivered.len());
}

#[test]
fn remote_protocol_rejects_wrong_version_and_policy_denial() {
    let mut wrong_version = summary_request();
    wrong_version.protocol_version = "axon.remote.v0".to_owned();
    assert_eq!(
        runtime().handle(&wrong_version),
        Err(AxonError::runtime_failure("远控协议版本不匹配"))
    );

    let denied_policy = RemoteCapabilityPolicy {
        allow_bounded_summary: false,
        allow_attachment_upload: true,
        allow_output_ref_grant: true,
        allow_thread_control: true,
    };
    assert_eq!(
        RemoteProtocolSimulatorRuntime::new(denied_policy).handle(&summary_request()),
        Err(AxonError::RemoteDenied {
            action: RemoteAction::ReadBoundedSummary,
            reason: RemoteDenialReason::NotGrantedByPolicy,
        })
    );
}

fn runtime() -> RemoteProtocolSimulatorRuntime {
    RemoteProtocolSimulatorRuntime::new(RemoteCapabilityPolicy::default_sidecar())
}

fn base_request() -> RemoteProtocolRequest {
    RemoteProtocolRequest {
        protocol_version: REMOTE_PROTOCOL_VERSION.to_owned(),
        client_id: "mobile-local-sim".to_owned(),
        request_id: "request-1".to_owned(),
        payload: RemoteProtocolPayload::ReadSummary {
            session_id: SessionId(11),
        },
    }
}

fn summary_request() -> RemoteProtocolRequest {
    base_request()
}

fn notification_request() -> RemoteProtocolRequest {
    let mut request = base_request();
    request.request_id = "request-notification".to_owned();
    request.payload = RemoteProtocolPayload::SendNotification {
        notification: Box::new(lifecycle_notification(
            SessionId(11),
            RemoteNotificationKind::TaskStarted,
            WireLifecycleStatus::Started,
            "Agent 状态更新",
            "当前任务仍在运行，远控只可查看摘要",
            &["thread:11"],
            RemoteNotificationSeverity::Info,
        )),
    };
    request
}

fn subscription_request() -> RemoteProtocolRequest {
    let mut request = base_request();
    request.request_id = "request-subscribe".to_owned();
    request.payload = RemoteProtocolPayload::SubscribeNotifications {
        subscription: full_subscription(SessionId(11), 7),
    };
    request
}

fn notification_batch_request() -> RemoteProtocolRequest {
    let mut request = base_request();
    request.request_id = "request-notification-batch".to_owned();
    request.payload = RemoteProtocolPayload::DeliverNotifications {
        subscription: full_subscription(SessionId(11), 7),
        notifications: vec![
            lifecycle_notification(
                SessionId(11),
                RemoteNotificationKind::TaskStarted,
                WireLifecycleStatus::Started,
                "任务开始",
                "Agent 线程已开始执行",
                &["thread:11"],
                RemoteNotificationSeverity::Info,
            ),
            lifecycle_notification(
                SessionId(11),
                RemoteNotificationKind::TestCompleted,
                WireLifecycleStatus::Completed,
                "测试完成",
                "cargo test 已完成，结果可通过 OutputRef 查看",
                &["output_ref:9"],
                RemoteNotificationSeverity::Info,
            ),
            lifecycle_notification(
                SessionId(11),
                RemoteNotificationKind::TaskFailed,
                WireLifecycleStatus::Failed,
                "任务失败",
                "当前 turn 失败，详情只开放 bounded summary",
                &["cell:17"],
                RemoteNotificationSeverity::Warning,
            ),
            lifecycle_notification(
                SessionId(11),
                RemoteNotificationKind::InputRequired,
                WireLifecycleStatus::Waiting,
                "需要输入",
                "Agent 需要用户补充确认信息",
                &["prompt:confirm"],
                RemoteNotificationSeverity::ActionRequired,
            ),
            lifecycle_notification(
                SessionId(11),
                RemoteNotificationKind::ThreadCompleted,
                WireLifecycleStatus::Completed,
                "线程完成",
                "远控线程已完成",
                &["thread:11"],
                RemoteNotificationSeverity::Info,
            ),
            lifecycle_notification(
                SessionId(11),
                RemoteNotificationKind::SafetyBlocked,
                WireLifecycleStatus::Blocked,
                "安全阻断",
                "远控请求被安全策略阻断",
                &["policy:network-denied"],
                RemoteNotificationSeverity::Warning,
            ),
            lifecycle_notification(
                SessionId(12),
                RemoteNotificationKind::TaskStarted,
                WireLifecycleStatus::Started,
                "其他会话",
                "该通知不应进入本订阅",
                &["thread:12"],
                RemoteNotificationSeverity::Info,
            ),
        ],
    };
    request
}

fn full_subscription(session_id: SessionId, max_batch: usize) -> RemoteNotificationSubscription {
    RemoteNotificationSubscription {
        session_id,
        subscriber_id: "mobile-local-sim".to_owned(),
        kinds: vec![
            RemoteNotificationKind::TaskStarted,
            RemoteNotificationKind::TestCompleted,
            RemoteNotificationKind::TaskFailed,
            RemoteNotificationKind::InputRequired,
            RemoteNotificationKind::ThreadCompleted,
            RemoteNotificationKind::SafetyBlocked,
        ],
        max_batch,
    }
}

fn lifecycle_notification(
    session_id: SessionId,
    kind: RemoteNotificationKind,
    status: WireLifecycleStatus,
    title: &str,
    bounded_body: &str,
    reference_ids: &[&str],
    severity: RemoteNotificationSeverity,
) -> RemoteNotificationProjection {
    RemoteNotificationProjection {
        session_id,
        kind,
        wire_event: wire_lifecycle_event(session_id, status, title),
        title: title.to_owned(),
        bounded_body: bounded_body.to_owned(),
        reference_ids: reference_ids
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        local_action_hint: "在本机打开对应线程或引用查看详情".to_owned(),
        severity,
    }
}

fn wire_lifecycle_event(
    session_id: SessionId,
    status: WireLifecycleStatus,
    title: &str,
) -> WireEventEnvelope {
    let event_key = format!("remote-notification-{}-{title}", session_id.0);
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
            title: title.to_owned(),
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

fn wire_lifecycle_status(event: &WireEventEnvelope) -> Option<WireLifecycleStatus> {
    match &event.kind {
        WireEventKind::TurnLifecycle(lifecycle) => Some(lifecycle.status),
        _ => None,
    }
}

fn assert_report(payload: &RemoteProtocolResponsePayload, channel: RemoteProtocolChannel) {
    let RemoteProtocolResponsePayload::DeliveryReport(report) = payload else {
        panic!("应返回本地投递报告");
    };
    assert_eq!(report.channel, channel);
    assert!(report.local_only);
}
