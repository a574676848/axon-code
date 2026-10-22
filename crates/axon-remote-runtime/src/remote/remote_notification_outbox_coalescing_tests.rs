//! outbox 合并与限流写入策略的定向回归。
//!
//! 合并键必须区分连接（chat）维度：同一 session/subscriber/reason 但不同 chat 的
//! 两条回复各自独立，绝不能被合并掉。

use std::path::PathBuf;

use axon_domain::SessionId;

use crate::{
    RemoteNotificationDelivery, RemoteNotificationFanoutPlan, RemoteNotificationKind,
    RemoteNotificationSeverity, RemoteTransportKind, WireLifecycleStatus,
    remote_notification_wire_lifecycle_event,
};

use super::super::*;

#[test]
fn outbox_coalesces_duplicate_active_notifications_inside_window() {
    let path = test_store_path("coalesced_notifications");
    let mut first = record(
        "dispatch-coalesce-1",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    first.session_id = 42;
    first.subscriber_id = "subscriber-1".into();
    first.reason = "goal.waiting".into();
    first.updated_at_ms = 100;
    RemoteNotificationOutboxRuntime::append_coalesced_record(&path, &first, 50).expect("first");
    let mut second = record(
        "dispatch-coalesce-2",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    second.session_id = 42;
    second.subscriber_id = "subscriber-1".into();
    second.reason = "goal.waiting".into();
    second.updated_at_ms = 120;
    let report = RemoteNotificationOutboxRuntime::append_coalesced_record(&path, &second, 50)
        .expect("merge");
    assert_eq!(
        report.coalesced_dispatch_id.as_deref(),
        Some(first.dispatch_id.as_str())
    );
    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].delivery_count,
        first.delivery_count + second.delivery_count
    );
    assert_eq!(records[0].dispatch_id, first.dispatch_id);
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_does_not_coalesce_notifications_across_connections() {
    let path = test_store_path("coalesce_across_connections");
    let mut first = record(
        "dispatch-chat-a",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    first.session_id = 42;
    first.subscriber_id = "subscriber-1".into();
    first.connection_id = "oc_chat_a".into();
    first.reason = "daemon:feishu_turn_completed".into();
    first.updated_at_ms = 100;
    RemoteNotificationOutboxRuntime::append_coalesced_rate_limited_record(
        &path,
        &first,
        1_000,
        RemoteNotificationThrottlePolicy {
            window_ms: 1_000,
            max_active_records: 64,
        },
    )
    .expect("first");

    // 同一 session/subscriber/reason 但不同 chat：两条回复必须各自保留，绝不能被合并掉。
    let mut second = record(
        "dispatch-chat-b",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    second.session_id = 42;
    second.subscriber_id = "subscriber-1".into();
    second.connection_id = "oc_chat_b".into();
    second.reason = "daemon:feishu_turn_completed".into();
    second.updated_at_ms = 120;
    let report = RemoteNotificationOutboxRuntime::append_coalesced_rate_limited_record(
        &path,
        &second,
        1_000,
        RemoteNotificationThrottlePolicy {
            window_ms: 1_000,
            max_active_records: 64,
        },
    )
    .expect("second");

    assert_eq!(report.coalesced_dispatch_id, None);
    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("records");
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|item| item.dispatch_id == first.dispatch_id)
    );
    assert!(
        records
            .iter()
            .any(|item| item.dispatch_id == second.dispatch_id)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_rate_limit_defers_without_dropping_unique_notification() {
    let path = test_store_path("rate_limited_notification");
    let mut first = record(
        "dispatch-rate-1",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    first.session_id = 42;
    first.subscriber_id = "subscriber-rate".into();
    first.updated_at_ms = 100;
    RemoteNotificationOutboxRuntime::append_record(&path, &first).expect("first");
    let mut second = record(
        "dispatch-rate-2",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    second.session_id = 42;
    second.subscriber_id = "subscriber-rate".into();
    second.updated_at_ms = 101;
    let report = RemoteNotificationOutboxRuntime::append_rate_limited_record(
        &path,
        &second,
        RemoteNotificationThrottlePolicy {
            window_ms: 1_000,
            max_active_records: 1,
        },
    )
    .expect("defer");
    assert!(report.evicted_dispatch_ids.is_empty());
    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("records");
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.dispatch_id == second.dispatch_id
                && record.next_retry_after_ms == Some(1_101))
    );
    let _ = std::fs::remove_file(path);
}

fn record(
    dispatch_id: &str,
    status: RemoteNotificationDispatchStatus,
    next_retry_after_ms: Option<u64>,
) -> RemoteNotificationOutboxRecord {
    RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan: plan(),
        status,
        recorded_at_ms: 100,
        updated_at_ms: 200,
        attempt_count: u32::from(status == RemoteNotificationDispatchStatus::Failed),
        next_retry_after_ms,
        last_error: None,
        adapter_message_id: None,
        source_trace: "remote_notification_outbox:test".to_owned(),
        reason: REMOTE_NOTIFICATION_OUTBOX_REASON_PLANNED.to_owned(),
    })
    .expect("record")
}

fn plan() -> RemoteNotificationFanoutPlan {
    RemoteNotificationFanoutPlan {
        session_id: SessionId(12),
        subscriber_id: "mobile-client".to_owned(),
        connection_id: "mobile-conn".to_owned(),
        pairing_id: "pairing-local".to_owned(),
        transport: RemoteTransportKind::Mobile,
        subscribed_kinds: vec![RemoteNotificationKind::TaskStarted],
        delivered: vec![RemoteNotificationDelivery {
            kind: RemoteNotificationKind::TaskStarted,
            wire_event: remote_notification_wire_lifecycle_event(
                SessionId(12),
                "remote-outbox-test",
                WireLifecycleStatus::Started,
                "远控通知",
                None,
                100,
            ),
            bounded_body: "通知正文只包含 bounded summary".to_owned(),
            reference_ids: vec!["thread:12".to_owned()],
            local_action_hint: "在本机打开对应线程".to_owned(),
            severity: RemoteNotificationSeverity::Info,
        }],
        dropped_count: 0,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
    }
}

fn test_store_path(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon_remote_notification_outbox_coalescing_{name}_{}.sqlite3",
        std::process::id()
    ))
}
