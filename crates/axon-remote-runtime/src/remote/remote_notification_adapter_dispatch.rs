//! 远控通知 adapter dispatch 批次计划。
//!
//! 该模块只把 outbox pending 记录投影为本机 adapter worker 可消费的有界批次，并生成 ack 记录。

#[path = "remote_notification_adapter_envelope.rs"]
pub(crate) mod envelope;
#[path = "remote_notification_adapter_feedback.rs"]
pub(crate) mod feedback;
#[path = "remote_notification_adapter_send.rs"]
pub(crate) mod send;

use std::path::Path;

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS, REMOTE_NOTIFICATION_OUTBOX_REASON_NETWORK_CLOSED,
    REMOTE_NOTIFICATION_OUTBOX_REASON_RAW_PAYLOAD_CLOSED,
    REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED, REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
    RemoteNotificationDispatchStatus, RemoteNotificationKind, RemoteNotificationOutboxRecord,
    RemoteNotificationOutboxRuntime, RemoteTransportKind,
};

/// 远控通知 adapter dispatch 已规划原因。
pub const REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_PLANNED: &str =
    "remote_notification_adapter_dispatch:planned";
/// 远控通知 adapter dispatch ack 已生成原因。
pub const REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_ACK_PLANNED: &str =
    "remote_notification_adapter_dispatch:ack_planned";
/// 远控通知 adapter dispatch 无 pending 原因。
pub const REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_NO_PENDING: &str =
    "remote_notification_adapter_dispatch:no_pending";
/// 远控通知 adapter dispatch ack reason。
pub const REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_ACCEPTED: &str =
    "remote_notification_adapter_dispatch:accepted";

/// 远控通知 adapter dispatch 单项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterDispatchItem {
    /// dispatch id。
    pub dispatch_id: String,
    /// 订阅者 ID。
    pub subscriber_id: String,
    /// 连接 ID。
    pub connection_id: String,
    /// 配对 ID。
    pub pairing_id: String,
    /// 投递数量。
    pub delivery_count: usize,
    /// 投递类别。
    pub delivery_kinds: Vec<RemoteNotificationKind>,
    /// 已尝试次数。
    pub attempt_count: u32,
}

/// 远控通知 adapter dispatch 批次计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterDispatchBatch {
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 本批次 dispatch 项。
    pub items: Vec<RemoteNotificationAdapterDispatchItem>,
    /// adapter 接受后应追加的 ack 记录。
    pub ack_records: Vec<RemoteNotificationOutboxRecord>,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控通知 adapter dispatch runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationAdapterDispatchRuntime;

impl RemoteNotificationAdapterDispatchRuntime {
    /// 从 outbox pending 生成 adapter worker 批次和 ack 记录。
    ///
    /// # Errors
    ///
    /// 当 session、时间、limit 或 outbox 记录无效时返回错误。
    pub fn plan_from_outbox(
        path: impl AsRef<Path>,
        session_id: SessionId,
        transport: RemoteTransportKind,
        now_ms: u64,
        accepted_at_ms: u64,
        limit: usize,
    ) -> AxonResult<RemoteNotificationAdapterDispatchBatch> {
        if session_id.0 == 0 || now_ms == 0 || accepted_at_ms < now_ms {
            return Err(AxonError::runtime_failure(
                "远控通知 adapter dispatch 时间或 session 无效",
            ));
        }
        if limit == 0 || limit > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS {
            return Err(AxonError::runtime_failure(
                "远控通知 adapter dispatch batch limit 无效",
            ));
        }
        let pending = RemoteNotificationOutboxRuntime::pending_for_transport(
            path, session_id, transport, now_ms,
        )?;
        let selected = pending.records.into_iter().take(limit).collect::<Vec<_>>();
        let items = selected.iter().map(dispatch_item).collect::<Vec<_>>();
        let ack_records = selected
            .iter()
            .map(|record| ack_record(record, accepted_at_ms))
            .collect::<AxonResult<Vec<_>>>()?;
        Ok(RemoteNotificationAdapterDispatchBatch {
            session_id,
            transport,
            items,
            ack_records,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: batch_reasons(selected.is_empty()),
        })
    }
}

fn dispatch_item(record: &RemoteNotificationOutboxRecord) -> RemoteNotificationAdapterDispatchItem {
    RemoteNotificationAdapterDispatchItem {
        dispatch_id: record.dispatch_id.clone(),
        subscriber_id: record.subscriber_id.clone(),
        connection_id: record.connection_id.clone(),
        pairing_id: record.pairing_id.clone(),
        delivery_count: record.delivery_count,
        delivery_kinds: record.delivery_kinds.clone(),
        attempt_count: record.attempt_count,
    }
}

fn ack_record(
    record: &RemoteNotificationOutboxRecord,
    accepted_at_ms: u64,
) -> AxonResult<RemoteNotificationOutboxRecord> {
    if accepted_at_ms <= record.updated_at_ms {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter dispatch ack 时间无效",
        ));
    }
    let mut ack = record.clone();
    ack.status = RemoteNotificationDispatchStatus::Attempting;
    ack.updated_at_ms = accepted_at_ms;
    ack.next_retry_after_ms = None;
    ack.last_error = None;
    ack.attempt_started_at_ms = Some(accepted_at_ms);
    ack.attempt_owner = Some(format!("pid:{}", std::process::id()));
    ack.recovery_marker = None;
    crate::REMOTE_NOTIFICATION_OUTBOX_REASON_ATTEMPTING.clone_into(&mut ack.reason);
    if ack.reason.chars().count() > REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter dispatch ack reason 无效",
        ));
    }
    Ok(ack)
}

fn batch_reasons(empty: bool) -> Vec<String> {
    let mut reasons = vec![
        REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED.to_owned(),
        REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_PLANNED.to_owned(),
        REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_ACK_PLANNED.to_owned(),
        REMOTE_NOTIFICATION_OUTBOX_REASON_NETWORK_CLOSED.to_owned(),
        REMOTE_NOTIFICATION_OUTBOX_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
    ];
    if empty {
        reasons.push(REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_NO_PENDING.to_owned());
    }
    reasons
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::{
        RemoteNotificationDelivery, RemoteNotificationFanoutPlan, RemoteNotificationOutboxInput,
        RemoteNotificationSeverity, WireLifecycleStatus, remote_notification_wire_lifecycle_event,
    };

    use super::*;

    #[test]
    fn adapter_dispatch_plans_batch_and_ack_without_network() {
        let path = test_store_path("plans_batch_and_ack");
        let record = outbox_record("dispatch-adapter");
        RemoteNotificationOutboxRuntime::append_record(&path, &record).expect("append");

        let batch = RemoteNotificationAdapterDispatchRuntime::plan_from_outbox(
            &path,
            SessionId(31),
            RemoteTransportKind::WebSocket,
            200,
            300,
            4,
        )
        .expect("batch");

        assert_eq!(batch.items.len(), 1);
        assert_eq!(batch.ack_records.len(), 1);
        assert_eq!(
            batch.ack_records[0].status,
            RemoteNotificationDispatchStatus::Attempting
        );
        assert!(batch.local_only);
        assert!(!batch.network_send_allowed);
        assert!(!batch.raw_payload_forward_allowed);
        let _ = fs::remove_file(path);
    }

    fn outbox_record(dispatch_id: &str) -> RemoteNotificationOutboxRecord {
        RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
            dispatch_id: dispatch_id.to_owned(),
            plan: fanout_plan(),
            status: RemoteNotificationDispatchStatus::Pending,
            recorded_at_ms: 100,
            updated_at_ms: 200,
            attempt_count: 0,
            next_retry_after_ms: None,
            last_error: None,
            adapter_message_id: None,
            source_trace: "remote_notification_adapter_dispatch:test".to_owned(),
            reason: REMOTE_NOTIFICATION_ADAPTER_DISPATCH_REASON_PLANNED.to_owned(),
        })
        .expect("record")
    }

    fn fanout_plan() -> RemoteNotificationFanoutPlan {
        RemoteNotificationFanoutPlan {
            session_id: SessionId(31),
            subscriber_id: "web-client".to_owned(),
            connection_id: "ws-conn".to_owned(),
            pairing_id: "pairing-local".to_owned(),
            transport: RemoteTransportKind::WebSocket,
            subscribed_kinds: vec![RemoteNotificationKind::TaskStarted],
            delivered: vec![RemoteNotificationDelivery {
                kind: RemoteNotificationKind::TaskStarted,
                wire_event: remote_notification_wire_lifecycle_event(
                    SessionId(31),
                    "remote-dispatch-test",
                    WireLifecycleStatus::Started,
                    "远控通知",
                    None,
                    100,
                ),
                bounded_body: "通知正文只包含 bounded summary".to_owned(),
                reference_ids: vec!["thread:31".to_owned()],
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
            "axon_remote_notification_adapter_dispatch_{name}_{}.jsonl",
            std::process::id()
        ))
    }
}
