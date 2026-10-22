use axon_domain::SessionId;

use crate::{
    RemoteNotificationDelivery, RemoteNotificationFanoutPlan, RemoteNotificationKind,
    RemoteNotificationOutboxInput, RemoteNotificationSeverity, RemoteTransportKind,
    WireLifecycleStatus, remote_notification_wire_lifecycle_event,
};

use super::*;

#[test]
fn timed_info_notification_defers_during_quiet_hours_without_dropping() {
    let path = test_store_path("timed_info_quiet_hours");
    let record = record(
        "timed-info-quiet",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    let report = RemoteNotificationOutboxRuntime::append_timed_record(
        &path,
        &record,
        RemoteNotificationSeverity::Info,
        RemoteNotificationTimingPolicy::default(),
        23 * 60,
    )
    .expect("静默期应写入延迟记录");
    assert!(report.evicted_dispatch_ids.is_empty());

    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("读取记录");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, RemoteNotificationDispatchStatus::Failed);
    assert_eq!(
        records[0].next_retry_after_ms,
        Some(record.updated_at_ms + RemoteNotificationTimingPolicy::default().quiet_retry_after_ms)
    );
    assert_eq!(records[0].delivery_count, record.delivery_count);
    let _ = std::fs::remove_file(path);
}

#[test]
fn timed_info_notification_uses_coalescing_and_rate_limit_outside_quiet_hours() {
    let path = test_store_path("timed_info_active_hours");
    let mut first = record(
        "timed-info-first",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    first.session_id = 42;
    first.subscriber_id = "timed-info".to_owned();
    first.reason = "progress.updated".to_owned();
    first.updated_at_ms = 100;
    RemoteNotificationOutboxRuntime::append_timed_record(
        &path,
        &first,
        RemoteNotificationSeverity::Info,
        RemoteNotificationTimingPolicy::default(),
        12 * 60,
    )
    .expect("首条通知");

    let mut second = record(
        "timed-info-second",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    second.session_id = first.session_id;
    second.subscriber_id = first.subscriber_id.clone();
    second.reason = first.reason.clone();
    second.updated_at_ms = 500;
    let report = RemoteNotificationOutboxRuntime::append_timed_record(
        &path,
        &second,
        RemoteNotificationSeverity::Info,
        RemoteNotificationTimingPolicy::default(),
        12 * 60,
    )
    .expect("第二条通知");
    assert_eq!(
        report.coalesced_dispatch_id.as_deref(),
        Some(first.dispatch_id.as_str())
    );

    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("读取记录");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].delivery_count,
        first.delivery_count + second.delivery_count
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn timed_user_reply_and_high_priority_notifications_ignore_quiet_hours() {
    // 用户直答不是主动通知：静默期内也必须立即出站，禁止 30 分钟延期。
    for (name, severity) in [
        ("direct-reply", RemoteNotificationSeverity::DirectReply),
        ("warning", RemoteNotificationSeverity::Warning),
        (
            "action-required",
            RemoteNotificationSeverity::ActionRequired,
        ),
    ] {
        let path = test_store_path(name);
        let record = record_with_severity(
            name,
            RemoteNotificationDispatchStatus::Pending,
            None,
            severity,
        );
        RemoteNotificationOutboxRuntime::append_timed_record(
            &path,
            &record,
            severity,
            RemoteNotificationTimingPolicy::default(),
            23 * 60,
        )
        .expect("高优先级通知");
        let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("读取记录");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, RemoteNotificationDispatchStatus::Pending);
        assert!(records[0].next_retry_after_ms.is_none());
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn timed_quiet_hours_coalescing_keeps_later_retry_deadline() {
    let path = test_store_path("timed_quiet_retry_max");
    let mut first = record(
        "timed-quiet-first",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    first.session_id = 42;
    first.subscriber_id = "timed-quiet".to_owned();
    first.reason = "progress.updated".to_owned();
    first.updated_at_ms = 100;
    let policy = RemoteNotificationTimingPolicy {
        quiet_retry_after_ms: 1_000,
        ..RemoteNotificationTimingPolicy::default()
    };
    RemoteNotificationOutboxRuntime::append_timed_record(
        &path,
        &first,
        RemoteNotificationSeverity::Info,
        policy,
        23 * 60,
    )
    .expect("首条静默通知");

    let mut second = record(
        "timed-quiet-second",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    second.session_id = first.session_id;
    second.subscriber_id = first.subscriber_id.clone();
    second.reason = first.reason.clone();
    second.updated_at_ms = 500;
    let report = RemoteNotificationOutboxRuntime::append_timed_record(
        &path,
        &second,
        RemoteNotificationSeverity::Info,
        RemoteNotificationTimingPolicy {
            quiet_retry_after_ms: 2_000,
            ..policy
        },
        23 * 60,
    )
    .expect("第二条静默通知");
    assert_eq!(
        report.coalesced_dispatch_id.as_deref(),
        Some(first.dispatch_id.as_str())
    );

    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("读取记录");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].next_retry_after_ms,
        Some(second.updated_at_ms + 2_000)
    );
    let _ = std::fs::remove_file(path);
}

fn record(
    dispatch_id: &str,
    status: RemoteNotificationDispatchStatus,
    next_retry_after_ms: Option<u64>,
) -> RemoteNotificationOutboxRecord {
    record_with_severity(
        dispatch_id,
        status,
        next_retry_after_ms,
        RemoteNotificationSeverity::Info,
    )
}

fn record_with_severity(
    dispatch_id: &str,
    status: RemoteNotificationDispatchStatus,
    next_retry_after_ms: Option<u64>,
    severity: RemoteNotificationSeverity,
) -> RemoteNotificationOutboxRecord {
    let mut plan = plan();
    plan.delivered[0].severity = severity;
    RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan,
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

fn test_store_path(name: &str) -> std::path::PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon_remote_notification_timing_{name}_{}.sqlite3",
        std::process::id()
    ))
}
