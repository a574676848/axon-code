//! durable 投递 retry soak 报告（R7-07）。
//!
//! 默认不参与常规测试；由 `tools/verify-jarvis-soak.ps1` 显式触发。
//! 每轮覆盖 append → claim(Attempting) → 退避重试 → 崩溃后 stale 恢复 → 最终投递，
//! 并断言未完成投递、尝试次数与保留记录数都不随轮次增长。

use std::{path::Path, time::Instant};

use crate::remote_notification_outbox::REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS;
use axon_domain::SessionId;

use crate::{
    DeliveryStore, REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS,
    REMOTE_NOTIFICATION_OUTBOX_REASON_PLANNED, RemoteNotificationDelivery,
    RemoteNotificationDispatchStatus, RemoteNotificationFanoutPlan, RemoteNotificationKind,
    RemoteNotificationOutboxInput, RemoteNotificationOutboxRecord, RemoteNotificationOutboxRuntime,
    RemoteNotificationSeverity, RemoteTransportKind, WireLifecycleStatus,
    remote_notification_wire_lifecycle_event,
};

const SOAK_CYCLES_ENV: &str = "JARVIS_DELIVERY_SOAK_CYCLES";
const DEFAULT_SOAK_CYCLES: usize = 1_000;
const SOAK_BASE_NOW_MS: u64 = 20_000_000;
const SOAK_CYCLE_MS: u64 = 1_000;
const SOAK_RETRY_BACKOFF_MS: u64 = 100;
const SOAK_STALE_AFTER_MS: u64 = 5_000;
const SOAK_DISPATCH_PREFIX: &str = "delivery-soak";
const SOAK_OWNER: &str = "delivery-soak-owner";
const SOAK_SESSION_ID: u64 = 12;
const SOAK_SUBSCRIBER_ID: &str = "soak-subscriber";
const SOAK_CONNECTION_ID: &str = "soak-connection";
const SOAK_PAIRING_ID: &str = "soak-pairing";
const SOAK_SOURCE_TRACE: &str = "remote_notification_outbox:soak";
const SOAK_FAILURE_SUMMARY: &str = "渠道瞬时失败，按退避重试";
const SOAK_BODY_DIGEST: &str = "soak 摘要";
const SOAK_REFERENCE_ID: &str = "thread:12";
const SOAK_ACTION_HINT: &str = "在本机查看 soak 进度";

#[test]
#[ignore = "常驻 soak 报告，由 tools/verify-jarvis-soak.ps1 触发"]
fn soak_report_for_delivery_retry_and_crash_recovery() {
    println!("{}", run_delivery_soak(delivery_soak_cycles()));
}

/// 读取 soak 轮次；默认 1000 轮。
fn delivery_soak_cycles() -> usize {
    std::env::var(SOAK_CYCLES_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_SOAK_CYCLES)
}

fn run_delivery_soak(cycles: usize) -> String {
    let directory = axon_test_support::test_temp_dir();
    let path = directory.path().join("remote_notification_outbox.sqlite3");
    let store = DeliveryStore::new(&path);
    let started = Instant::now();
    for cycle in 0..cycles {
        let cycle_id = format!("{SOAK_DISPATCH_PREFIX}-{cycle}");
        run_retry_cycle(&store, &format!("{cycle_id}-retry"), cycle);
        run_crash_recovery_cycle(&store, &format!("{cycle_id}-stale"), cycle);
    }
    let health = store.summary(SOAK_BASE_NOW_MS).expect("投递健康快照");
    assert_eq!(health.backlog.pending, 0, "soak 结束后仍有未完成投递记录");
    let records = RemoteNotificationOutboxRuntime::read_records(&path).expect("读取 outbox");
    assert!(
        records.len() <= REMOTE_NOTIFICATION_OUTBOX_MAX_RECORDS,
        "outbox 保留记录数 {} 超出上限",
        records.len()
    );
    let max_attempts = records
        .iter()
        .map(|record| record.attempt_count)
        .max()
        .unwrap_or(0);
    assert!(
        max_attempts <= REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS,
        "attempt_count {max_attempts} 超出上限"
    );
    let unfinished = records
        .iter()
        .filter(|record| {
            matches!(
                record.status,
                RemoteNotificationDispatchStatus::Pending
                    | RemoteNotificationDispatchStatus::Attempting
                    | RemoteNotificationDispatchStatus::Failed
            )
        })
        .count();
    assert_eq!(unfinished, 0, "soak 结束后仍有未终态投递记录");
    serde_json::json!({
        "cycles": cycles,
        "dispatches": cycles.saturating_mul(2),
        "duration_ms": started.elapsed().as_millis(),
        "retained_records": records.len(),
        "max_attempt_count": max_attempts,
        "unfinished_records": unfinished,
        "db_bytes": file_len(&path),
    })
    .to_string()
}

/// 退避重试路径：首次尝试失败后必须等待退避时间，再领取并投递成功。
fn run_retry_cycle(store: &DeliveryStore, dispatch_id: &str, cycle: usize) {
    let now_ms = cycle_now_ms(cycle);
    append_pending(store, dispatch_id, now_ms);
    let claimed = claim_one(store, now_ms);
    let mut failed = claimed.clone();
    failed.status = RemoteNotificationDispatchStatus::Failed;
    failed.attempt_count = claimed.attempt_count.saturating_add(1);
    failed.next_retry_after_ms = Some(now_ms.saturating_add(SOAK_RETRY_BACKOFF_MS));
    failed.last_error = Some(SOAK_FAILURE_SUMMARY.to_owned());
    failed.updated_at_ms = now_ms;
    store.retry(&failed).expect("写入可重试失败");

    assert_eq!(
        store
            .claim(RemoteTransportKind::Feishu, now_ms, 1, SOAK_OWNER)
            .expect("未到期不应领取")
            .len(),
        0,
        "退避未到期时不得重复投递"
    );
    let retry_at = now_ms.saturating_add(SOAK_RETRY_BACKOFF_MS);
    let reclaimed = claim_one(store, retry_at);
    assert_eq!(
        reclaimed.attempt_count,
        claimed.attempt_count.saturating_add(1),
        "重试必须递增尝试次数"
    );
    settle_delivered(store, &reclaimed, retry_at);
}

/// 崩溃恢复路径：Attempting 记录在 stale 窗口后被恢复为 Pending 并重新投递成功。
fn run_crash_recovery_cycle(store: &DeliveryStore, dispatch_id: &str, cycle: usize) {
    let now_ms = cycle_now_ms(cycle);
    append_pending(store, dispatch_id, now_ms);
    let claimed = claim_one(store, now_ms);
    let recovered_at = now_ms.saturating_add(SOAK_STALE_AFTER_MS);
    let recovered = store
        .recover_stale_attempts(recovered_at, SOAK_STALE_AFTER_MS)
        .expect("恢复 stale Attempting");
    assert_eq!(recovered, 1, "stale Attempting 必须被恢复为可投递");
    let reclaimed = claim_one(store, recovered_at);
    assert_eq!(
        reclaimed.attempt_count, claimed.attempt_count,
        "崩溃未写入结果时不得虚增尝试次数"
    );
    settle_delivered(store, &reclaimed, recovered_at);
}

fn append_pending(store: &DeliveryStore, dispatch_id: &str, now_ms: u64) {
    store
        .append(&pending_record(dispatch_id, now_ms))
        .expect("追加待投递记录");
}

fn claim_one(store: &DeliveryStore, now_ms: u64) -> RemoteNotificationOutboxRecord {
    let claimed = store
        .claim(RemoteTransportKind::Feishu, now_ms, 1, SOAK_OWNER)
        .expect("领取待投递记录");
    assert_eq!(claimed.len(), 1, "应当恰好领取一条待投递记录");
    assert_eq!(
        claimed[0].status,
        RemoteNotificationDispatchStatus::Attempting
    );
    claimed.into_iter().next().expect("领取记录")
}

fn settle_delivered(store: &DeliveryStore, record: &RemoteNotificationOutboxRecord, now_ms: u64) {
    let mut delivered = record.clone();
    delivered.status = RemoteNotificationDispatchStatus::Delivered;
    delivered.updated_at_ms = now_ms.saturating_add(1);
    delivered.attempt_started_at_ms = None;
    delivered.attempt_owner = None;
    store.settle(&delivered).expect("结算投递成功");
}

fn cycle_now_ms(cycle: usize) -> u64 {
    SOAK_BASE_NOW_MS.saturating_add(cycle as u64 * SOAK_CYCLE_MS)
}

fn pending_record(dispatch_id: &str, now_ms: u64) -> RemoteNotificationOutboxRecord {
    RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
        dispatch_id: dispatch_id.to_owned(),
        plan: soak_plan(now_ms),
        status: RemoteNotificationDispatchStatus::Pending,
        recorded_at_ms: now_ms,
        updated_at_ms: now_ms,
        attempt_count: 0,
        next_retry_after_ms: None,
        last_error: None,
        adapter_message_id: None,
        source_trace: SOAK_SOURCE_TRACE.to_owned(),
        reason: REMOTE_NOTIFICATION_OUTBOX_REASON_PLANNED.to_owned(),
    })
    .expect("构造待投递记录")
}

fn soak_plan(now_ms: u64) -> RemoteNotificationFanoutPlan {
    RemoteNotificationFanoutPlan {
        session_id: SessionId(SOAK_SESSION_ID),
        subscriber_id: SOAK_SUBSCRIBER_ID.to_owned(),
        connection_id: SOAK_CONNECTION_ID.to_owned(),
        pairing_id: SOAK_PAIRING_ID.to_owned(),
        transport: RemoteTransportKind::Feishu,
        subscribed_kinds: vec![RemoteNotificationKind::TaskStarted],
        delivered: vec![RemoteNotificationDelivery {
            kind: RemoteNotificationKind::TaskStarted,
            wire_event: remote_notification_wire_lifecycle_event(
                SessionId(SOAK_SESSION_ID),
                SOAK_SOURCE_TRACE,
                WireLifecycleStatus::Started,
                SOAK_BODY_DIGEST,
                None,
                i64::try_from(now_ms).expect("时间戳"),
            ),
            bounded_body: SOAK_BODY_DIGEST.to_owned(),
            reference_ids: vec![SOAK_REFERENCE_ID.to_owned()],
            local_action_hint: SOAK_ACTION_HINT.to_owned(),
            severity: RemoteNotificationSeverity::Info,
        }],
        dropped_count: 0,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
    }
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}
