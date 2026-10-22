use std::path::PathBuf;

use axon_domain::SessionId;

use crate::{
    DeliveryStore, RemoteNotificationDelivery, RemoteNotificationFanoutPlan,
    RemoteNotificationKind, RemoteNotificationSeverity, RemoteTransportKind, WireLifecycleStatus,
    remote_notification_wire_lifecycle_event,
};

use super::*;

/// 收敛必须保留头部与尾部：头部是错误码/摘要，尾部是 `log_id` 与排查链接。
/// 只截尾会丢掉定位所需的 `log_id`，因此中间用省略标记替代。
#[test]
fn clamp_keeps_head_and_tail_and_omits_the_middle() {
    let head = "code=99992402 msg=field validation failed";
    let tail = "log_id=202609240117227909F5E046064DAB8378";
    let body = "x".repeat(4_000);
    let long = format!("{head}{body}{tail}");

    // 用一个远小于原文的预算，让「中间被省略」成为可断言的事实。
    let budget = 256;
    let clamped = clamp_text_keeping_edges(&long, budget);

    assert_eq!(clamped.chars().count(), budget);
    assert!(clamped.starts_with(head), "头部必须完整保留：{clamped}");
    assert!(clamped.ends_with(tail), "尾部必须完整保留：{clamped}");
    assert!(clamped.contains(CLAMP_OMISSION), "中间必须出现省略标记");
    let filler_kept = clamped.matches('x').count();
    let available = budget - CLAMP_OMISSION.chars().count();
    assert!(
        filler_kept <= available,
        "填充字符不得超过头尾可用预算：{filler_kept} > {available}"
    );
    assert!(
        long.chars().count() - clamped.chars().count() > 3_000,
        "原有 4000 字符的中间内容必须被省略"
    );
}

#[test]
fn clamp_leaves_within_budget_text_untouched() {
    let short = "Feishu message API failed";
    assert_eq!(
        clamp_text_keeping_edges(short, REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS),
        short
    );
}

/// 真机事故守卫：飞书 OpenAPI 的错误体（含 field_violations/log_id/troubleshooter
/// 链接）远超原先 240 字符的上界，导致 adapter outcome 被判为「错误摘要无效」，
/// 失败连记录都落不下来、重试通道整体失效。上界必须容纳完整诊断信息。
///
/// 断言在常量上下文完成：预算一旦被调小，编译期即失败，而不是等到真机上默默失去重试。
const _: () = assert!(REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS >= 1_024);

#[test]
fn outbox_persists_pending_dispatch_and_filters_retry() {
    let path = test_store_path("pending_dispatch");
    let planned = record(
        "dispatch-planned",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    let retry = record(
        "dispatch-retry",
        RemoteNotificationDispatchStatus::Failed,
        Some(300),
    );
    RemoteNotificationOutboxRuntime::append_record(&path, &planned).expect("append planned");
    RemoteNotificationOutboxRuntime::append_record(&path, &retry).expect("append retry");

    let pending = RemoteNotificationOutboxRuntime::pending_for_transport(
        &path,
        SessionId(12),
        RemoteTransportKind::Mobile,
        300,
    )
    .expect("pending");

    assert_eq!(pending.records.len(), 2);
    assert!(!pending.network_send_allowed);
    assert!(!pending.raw_payload_forward_allowed);
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_pending_folds_latest_dispatch_status() {
    let path = test_store_path("folds_latest_dispatch_status");
    let planned = record(
        "dispatch-folded",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    let mut accepted = record(
        "dispatch-folded",
        RemoteNotificationDispatchStatus::Attempting,
        None,
    );
    accepted.updated_at_ms = 300;
    RemoteNotificationOutboxRuntime::append_record(&path, &planned).expect("append planned");
    RemoteNotificationOutboxRuntime::append_record(&path, &accepted).expect("append accepted");

    let latest = RemoteNotificationOutboxRuntime::latest_records(&path).expect("latest");
    let pending = RemoteNotificationOutboxRuntime::pending_for_transport(
        &path,
        SessionId(12),
        RemoteTransportKind::Mobile,
        300,
    )
    .expect("pending");

    assert_eq!(latest.len(), 1);
    assert_eq!(
        latest[0].status,
        RemoteNotificationDispatchStatus::Attempting
    );
    assert!(pending.records.is_empty());
    assert!(
        pending
            .reasons
            .contains(&REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED.to_owned())
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_pending_orders_same_millisecond_records_by_source_sequence() {
    let path = test_store_path("same_millisecond_sequence_order");
    // 同一毫秒内，dispatch_id 的哈希序与事件先后相反：次序键必须由序号决定。
    let mut earlier = record(
        "dispatch-b",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    earlier.session_id = 12;
    earlier.sequence = Some(1);
    earlier.updated_at_ms = 500;
    let mut later = record(
        "dispatch-a",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    later.session_id = 12;
    later.sequence = Some(2);
    later.updated_at_ms = 500;
    // 逆序写入，确保结果不依赖写入顺序。
    RemoteNotificationOutboxRuntime::append_record(&path, &later).expect("later");
    RemoteNotificationOutboxRuntime::append_record(&path, &earlier).expect("earlier");

    let pending = RemoteNotificationOutboxRuntime::pending_for_transport(
        &path,
        SessionId(12),
        RemoteTransportKind::Mobile,
        500,
    )
    .expect("pending");

    assert_eq!(
        pending
            .records
            .iter()
            .map(|item| item.dispatch_id.as_str())
            .collect::<Vec<_>>(),
        vec!["dispatch-b", "dispatch-a"]
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_rejects_unsafe_flags() {
    let path = test_store_path("rejects_unsafe");
    let mut unsafe_record = record(
        "dispatch-unsafe",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    unsafe_record.network_send_allowed = true;

    assert!(RemoteNotificationOutboxRuntime::append_record(&path, &unsafe_record).is_err());
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_recovers_stale_attempt_with_explicit_marker() {
    let path = test_store_path("recovers_stale_attempt");
    let pending = record(
        "dispatch-recover",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    RemoteNotificationOutboxRuntime::append_record(&path, &pending).expect("append pending");
    RemoteNotificationOutboxRuntime::mark_attempting(&path, "dispatch-recover", 300, "pid:1")
        .expect("mark attempting");

    let recovered =
        RemoteNotificationOutboxRuntime::recover_stale_attempts(&path, 500, 100).expect("recover");
    assert_eq!(recovered, 1);
    let latest = RemoteNotificationOutboxRuntime::latest_records(&path).expect("latest");
    assert_eq!(latest[0].status, RemoteNotificationDispatchStatus::Pending);
    assert!(latest[0].recovery_marker.is_some());
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_attempt_checkpoint_advances_revision_when_started_in_the_same_millisecond() {
    let path = test_store_path("attempt_checkpoint_same_millisecond");
    let pending = record(
        "dispatch-same-millisecond",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    RemoteNotificationOutboxRuntime::append_record(&path, &pending).expect("append pending");

    let checkpoint = RemoteNotificationOutboxRuntime::mark_attempting(
        &path,
        "dispatch-same-millisecond",
        pending.updated_at_ms,
        "pid:1",
    )
    .expect("mark attempting");
    let latest = RemoteNotificationOutboxRuntime::latest_records(&path).expect("latest");

    assert_eq!(
        checkpoint.attempt_started_at_ms,
        Some(pending.updated_at_ms)
    );
    assert!(checkpoint.updated_at_ms > pending.updated_at_ms);
    assert_eq!(latest, vec![checkpoint]);
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_retention_removes_only_expired_terminal_records() {
    const RETENTION_CUTOFF_MS: u64 = 300;

    let path = test_store_path("retention");
    let mut old_delivered = record(
        "dispatch-old-delivered",
        RemoteNotificationDispatchStatus::Delivered,
        None,
    );
    old_delivered.updated_at_ms = 100;
    let mut recent_abandoned = record(
        "dispatch-recent-abandoned",
        RemoteNotificationDispatchStatus::Abandoned,
        None,
    );
    recent_abandoned.updated_at_ms = 400;
    let pending = record(
        "dispatch-pending",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    let retryable = record(
        "dispatch-retryable",
        RemoteNotificationDispatchStatus::Failed,
        Some(500),
    );
    for record in [&old_delivered, &recent_abandoned, &pending, &retryable] {
        RemoteNotificationOutboxRuntime::append_record(&path, record).expect("append record");
    }

    assert_eq!(
        RemoteNotificationOutboxRuntime::remove_terminal_records_before(
            &path,
            RETENTION_CUTOFF_MS,
        )
        .expect("remove terminal records"),
        1
    );
    let remaining = RemoteNotificationOutboxRuntime::latest_records(&path).expect("read records");
    assert_eq!(remaining.len(), 3);
    assert!(
        remaining
            .iter()
            .all(|record| record.dispatch_id != old_delivered.dispatch_id)
    );
    assert!(
        remaining
            .iter()
            .any(|record| record.dispatch_id == recent_abandoned.dispatch_id)
    );
    assert!(
        remaining
            .iter()
            .any(|record| record.dispatch_id == pending.dispatch_id)
    );
    assert!(
        remaining
            .iter()
            .any(|record| record.dispatch_id == retryable.dispatch_id)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn outbox_capacity_keeps_new_dispatch_and_evicts_terminal_before_active_record() {
    let path = test_store_path("capacity_keeps_new_dispatch");
    let active_dispatch_id = "dispatch-old-active";
    let terminal_dispatch_id = "dispatch-terminal";
    let mut active = record(
        active_dispatch_id,
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    active.updated_at_ms = 100;
    RemoteNotificationOutboxRuntime::append_record(&path, &active).expect("append active");

    for index in 1..REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS - 1 {
        let mut record = record(
            &format!("dispatch-pending-{index}"),
            RemoteNotificationDispatchStatus::Pending,
            None,
        );
        record.updated_at_ms = 100 + u64::try_from(index).expect("index fits u64");
        RemoteNotificationOutboxRuntime::append_record(&path, &record).expect("append pending");
    }

    let mut terminal = record(
        terminal_dispatch_id,
        RemoteNotificationDispatchStatus::Delivered,
        None,
    );
    terminal.updated_at_ms = 1_000;
    RemoteNotificationOutboxRuntime::append_record(&path, &terminal).expect("append terminal");

    let mut newest = record(
        "dispatch-newest",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    newest.updated_at_ms = 2_000;
    let report =
        RemoteNotificationOutboxRuntime::append_record(&path, &newest).expect("append newest");
    let records = RemoteNotificationOutboxRuntime::latest_records(&path).expect("read records");

    assert_eq!(records.len(), REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS);
    assert_eq!(report.evicted_dispatch_ids, vec![terminal_dispatch_id]);
    assert!(
        records
            .iter()
            .any(|record| record.dispatch_id == "dispatch-newest")
    );
    assert!(
        records
            .iter()
            .any(|record| record.dispatch_id == active_dispatch_id)
    );
    assert!(
        records
            .iter()
            .all(|record| record.dispatch_id != terminal_dispatch_id)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn quiet_hours_defers_without_dropping_notification() {
    let path = test_store_path("quiet_hours");
    let record = record(
        "quiet-hours-1",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    let report = RemoteNotificationOutboxRuntime::append_quiet_hours_record(
        &path,
        &record,
        22 * 60,
        7 * 60,
        23 * 60,
        9_999,
    )
    .expect("append quiet-hours record");
    assert!(report.evicted_dispatch_ids.is_empty());
    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("read records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].dispatch_id, "quiet-hours-1");
    assert_eq!(records[0].next_retry_after_ms, Some(9_999));
    assert_eq!(records[0].delivery_count, record.delivery_count);
}

#[test]
fn outbox_capacity_evicts_oldest_active_record_when_no_terminal_record_exists() {
    let path = test_store_path("capacity_preserves_active_records");
    for index in 0..REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS {
        let mut pending = record(
            &format!("dispatch-pending-{index}"),
            RemoteNotificationDispatchStatus::Pending,
            None,
        );
        pending.updated_at_ms = 100 + u64::try_from(index).expect("index fits u64");
        RemoteNotificationOutboxRuntime::append_record(&path, &pending).expect("append pending");
    }

    let newest = record(
        "dispatch-newest",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    let report =
        RemoteNotificationOutboxRuntime::append_record(&path, &newest).expect("append newest");
    let records = RemoteNotificationOutboxRuntime::latest_records(&path).expect("read records");

    assert_eq!(records.len(), REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS);
    assert_eq!(report.evicted_dispatch_ids, vec!["dispatch-pending-0"]);
    assert!(
        records
            .iter()
            .any(|record| record.dispatch_id == newest.dispatch_id)
    );
    assert!(
        records
            .iter()
            .all(|record| record.dispatch_id != "dispatch-pending-0")
    );
    assert!(
        records
            .iter()
            .all(|record| record.status == RemoteNotificationDispatchStatus::Pending)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn delivery_store_deduplicates_and_claims_each_dispatch_once_with_a_bound() {
    let path = test_store_path("delivery_store_deduplicated_claim");
    let store = DeliveryStore::new(path.clone());
    let mut first = record(
        "delivery-store-first",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    first.reason = "delivery.waiting".into();
    first.updated_at_ms = 210;
    let mut duplicate = record(
        "delivery-store-duplicate",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    duplicate.reason = first.reason.clone();
    duplicate.updated_at_ms = 220;
    let mut second = record(
        "delivery-store-second",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    second.reason = "delivery.completed".into();
    second.updated_at_ms = 230;

    store.append_deduplicated(&first, 50).expect("append first");
    let report = store
        .append_deduplicated(&duplicate, 50)
        .expect("deduplicate duplicate");
    store.append(&second).expect("append second");
    assert_eq!(
        report.coalesced_dispatch_id.as_deref(),
        Some(first.dispatch_id.as_str())
    );

    let first_claim = store
        .claim(RemoteTransportKind::Mobile, 300, 1, "delivery-store:test")
        .expect("first bounded claim");
    let second_claim = store
        .claim(RemoteTransportKind::Mobile, 301, 1, "delivery-store:test")
        .expect("second bounded claim");
    let final_claim = store
        .claim(RemoteTransportKind::Mobile, 302, 1, "delivery-store:test")
        .expect("final bounded claim");

    assert_eq!(first_claim.len(), 1);
    assert_eq!(second_claim.len(), 1);
    assert!(final_claim.is_empty());
    assert_ne!(first_claim[0].dispatch_id, second_claim[0].dispatch_id);
    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("read records");
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.status == RemoteNotificationDispatchStatus::Attempting)
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn delivery_store_retries_only_when_due_and_before_attempt_limit() {
    let path = test_store_path("delivery_store_retry");
    let store = DeliveryStore::new(path.clone());
    let retry = record(
        "delivery-store-retry",
        RemoteNotificationDispatchStatus::Failed,
        Some(300),
    );
    store.retry(&retry).expect("persist retry");
    assert!(
        store
            .claim(RemoteTransportKind::Mobile, 299, 1, "delivery-store:test")
            .expect("claim before retry")
            .is_empty()
    );
    assert_eq!(
        store
            .claim(RemoteTransportKind::Mobile, 300, 1, "delivery-store:test")
            .expect("claim due retry")
            .len(),
        1
    );

    let mut exhausted = record(
        "delivery-store-exhausted",
        RemoteNotificationDispatchStatus::Failed,
        Some(400),
    );
    exhausted.attempt_count = REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS;
    store.append(&exhausted).expect("persist exhausted record");
    assert!(store.retry(&exhausted).is_err());
    assert!(
        store
            .claim(RemoteTransportKind::Mobile, 400, 1, "delivery-store:test")
            .expect("claim exhausted retry")
            .is_empty()
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn delivery_store_settles_claim_without_execution_lease() {
    let path = test_store_path("delivery_store_settlement");
    let store = DeliveryStore::new(path.clone());
    let pending = record(
        "delivery-store-settlement",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    store.append(&pending).expect("append pending");
    let mut claimed = store
        .claim(RemoteTransportKind::Mobile, 300, 1, "delivery-store:test")
        .expect("claim pending")
        .pop()
        .expect("claimed record");
    claimed.status = RemoteNotificationDispatchStatus::Delivered;
    claimed.updated_at_ms = 301;
    claimed.attempt_started_at_ms = None;
    claimed.attempt_owner = None;
    store.settle(&claimed).expect("settle delivery");

    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("read records");
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0].status,
        RemoteNotificationDispatchStatus::Delivered
    );
    assert!(records[0].attempt_owner.is_none());
    let _ = std::fs::remove_file(path);
}

#[test]
fn delivery_store_claims_across_other_transports_without_starvation() {
    let path = test_store_path("delivery_store_starvation");
    let store = DeliveryStore::new(path.clone());
    // 写入 3 条属于 Mobile 传输的 pending 记录
    for index in 0..3 {
        let other = record(
            &format!("delivery-other-{index}"),
            RemoteNotificationDispatchStatus::Pending,
            None,
        );
        store.append(&other).expect("append other");
    }
    // 写入 1 条属于 Feishu 传输的 pending 记录
    let mut feishu_record = record(
        "delivery-feishu-target",
        RemoteNotificationDispatchStatus::Pending,
        None,
    );
    feishu_record.transport = RemoteTransportKind::Feishu;
    feishu_record.updated_at_ms = 250;
    store.append(&feishu_record).expect("append target");

    // 即使 limit 仅为 1（小于前面其他 transport 的记录数 3），也必须跨过其他记录成功领取到 Feishu 目标记录
    let claimed = store
        .claim(RemoteTransportKind::Feishu, 300, 1, "delivery-store:test")
        .expect("claim target without starvation");
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].dispatch_id, "delivery-feishu-target");
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

fn test_store_path(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon_remote_notification_outbox_{name}_{}.sqlite3",
        std::process::id()
    ))
}
