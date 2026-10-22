//! 远控通知 adapter 发送结果归一化。
//!
//! 该模块接收本地 adapter 对 envelope 的发送结果，生成可追加到 outbox 的状态记录；不直接发送网络。

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    RemoteNotificationAdapterEnvelopeReport, RemoteNotificationDispatchStatus,
    RemoteNotificationOutboxRecord, RemoteTransportKind,
};

use crate::REMOTE_NOTIFICATION_OUTBOX_ADAPTER_MESSAGE_ID_MAX_CHARS;

#[path = "remote_notification_adapter_send_plan.rs"]
mod remote_notification_adapter_send_plan;

use remote_notification_adapter_send_plan::{
    accepted_by_dispatch, outcome_record, outcomes_by_dispatch, receipt, send_report,
    validate_match, validate_report,
};

/// 远控通知 adapter send message id 最大字符数。
pub const REMOTE_NOTIFICATION_ADAPTER_SEND_MESSAGE_ID_MAX_CHARS: usize =
    REMOTE_NOTIFICATION_OUTBOX_ADAPTER_MESSAGE_ID_MAX_CHARS;
/// 远控通知 adapter send 已规划原因。
pub const REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_PLANNED: &str =
    "remote_notification_adapter_send:planned";
/// 远控通知 adapter send 已投递原因。
pub const REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_DELIVERED: &str =
    "remote_notification_adapter_send:delivered";
/// 远控通知 adapter send 网络关闭原因。
pub const REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_NETWORK_CLOSED: &str =
    "remote_notification_adapter_send:network_closed";
/// 远控通知 adapter send raw payload 关闭原因。
pub const REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_RAW_PAYLOAD_CLOSED: &str =
    "remote_notification_adapter_send:raw_payload_closed";

/// 远控通知 adapter 发送结果状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteNotificationAdapterSendStatus {
    /// adapter 确认已投递。
    Delivered,
    /// adapter 返回可重试失败。
    RetryableFailure,
    /// adapter 返回永久失败。
    PermanentFailure,
}

/// 单个 envelope 的 adapter 发送结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterSendOutcome {
    /// dispatch id。
    pub dispatch_id: String,
    /// 发送结果状态。
    pub status: RemoteNotificationAdapterSendStatus,
    /// adapter 反馈时间。
    pub sent_at_ms: u64,
    /// 下次可重试时间。
    pub next_retry_after_ms: Option<u64>,
    /// adapter 返回的有界消息 ID。
    pub adapter_message_id: Option<String>,
    /// 有界错误摘要。
    pub error_summary: Option<String>,
    /// source trace。
    pub source_trace: String,
}

/// 远控通知 adapter send 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterSendInput {
    /// envelope 投影报告。
    pub envelope_report: RemoteNotificationAdapterEnvelopeReport,
    /// 已被 adapter 接受的 outbox 记录。
    pub accepted_records: Vec<RemoteNotificationOutboxRecord>,
    /// adapter 发送结果。
    pub outcomes: Vec<RemoteNotificationAdapterSendOutcome>,
}

/// 单个发送结果 receipt。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterSendReceipt {
    /// dispatch id。
    pub dispatch_id: String,
    /// 发送结果状态。
    pub status: RemoteNotificationAdapterSendStatus,
    /// 追加到 outbox 的状态。
    pub record_status: RemoteNotificationDispatchStatus,
    /// adapter 返回的有界消息 ID。
    pub adapter_message_id: Option<String>,
}

/// 远控通知 adapter send 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterSendReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 应追加到 outbox 的记录。
    pub records: Vec<RemoteNotificationOutboxRecord>,
    /// 发送 receipt。
    pub receipts: Vec<RemoteNotificationAdapterSendReceipt>,
    /// 已投递数量。
    pub delivered_count: usize,
    /// 可重试失败数量。
    pub retryable_failure_count: usize,
    /// 永久失败数量。
    pub permanent_failure_count: usize,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许 runtime 主动发送网络。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控通知 adapter send runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationAdapterSendRuntime;

impl RemoteNotificationAdapterSendRuntime {
    /// 把本地 adapter 发送结果归一化为 outbox 状态记录。
    ///
    /// # Errors
    ///
    /// 当 envelope、accepted record 或 outcome 不匹配，或反馈策略无效时返回错误。
    pub fn plan_send_result(
        input: RemoteNotificationAdapterSendInput,
    ) -> AxonResult<RemoteNotificationAdapterSendReport> {
        validate_report(&input.envelope_report)?;
        if input.outcomes.len() != input.envelope_report.envelopes.len()
            || input.accepted_records.len() != input.envelope_report.envelopes.len()
        {
            return Err(AxonError::runtime_failure(
                "远控通知 adapter send outcome 或 accepted record 数量不匹配",
            ));
        }
        let accepted = accepted_by_dispatch(input.accepted_records)?;
        let outcomes = outcomes_by_dispatch(input.outcomes)?;
        let mut records = Vec::with_capacity(input.envelope_report.envelopes.len());
        let mut receipts = Vec::with_capacity(input.envelope_report.envelopes.len());

        for envelope in &input.envelope_report.envelopes {
            let source = accepted.get(&envelope.dispatch_id).ok_or({
                AxonError::runtime_failure("远控通知 adapter send 缺少 accepted record")
            })?;
            let outcome = outcomes
                .get(&envelope.dispatch_id)
                .ok_or(AxonError::runtime_failure(
                    "远控通知 adapter send 缺少 outcome",
                ))?;
            validate_match(&input.envelope_report, envelope, source, outcome)?;
            let record = outcome_record(source, outcome)?;
            receipts.push(receipt(outcome, &record));
            records.push(record);
        }

        Ok(send_report(&input.envelope_report, records, receipts))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        RemoteNotificationAdapterDispatchBatch, RemoteNotificationAdapterDispatchItem,
        RemoteNotificationAdapterEnvelopeRuntime, RemoteNotificationDelivery,
        RemoteNotificationFanoutPlan, RemoteNotificationKind, RemoteNotificationOutboxInput,
        RemoteNotificationSeverity, WireLifecycleStatus, remote_notification_wire_lifecycle_event,
    };

    use super::*;

    #[test]
    fn adapter_send_projects_delivered_record_without_network() {
        let source = accepted_record("dispatch-send");
        let report = RemoteNotificationAdapterEnvelopeRuntime::project_batch(&batch())
            .expect("envelope report");

        let send = RemoteNotificationAdapterSendRuntime::plan_send_result(
            RemoteNotificationAdapterSendInput {
                envelope_report: report,
                accepted_records: vec![source],
                outcomes: vec![delivered_outcome("dispatch-send")],
            },
        )
        .expect("send report");

        assert_eq!(send.delivered_count, 1);
        assert_eq!(
            send.records[0].status,
            RemoteNotificationDispatchStatus::Delivered
        );
        assert_eq!(send.records[0].attempt_count, 1);
        assert_eq!(
            send.records[0].adapter_message_id.as_deref(),
            Some("ws-message-1")
        );
        assert!(send.local_only);
        assert!(!send.network_send_allowed);
        assert!(!send.raw_payload_forward_allowed);
    }

    #[test]
    fn adapter_send_rejects_retryable_without_retry_at() {
        let source = accepted_record("dispatch-send-retry");
        let report = RemoteNotificationAdapterEnvelopeRuntime::project_batch(&batch_with_id(
            "dispatch-send-retry",
        ))
        .expect("envelope report");
        let mut outcome = failure_outcome(
            "dispatch-send-retry",
            RemoteNotificationAdapterSendStatus::RetryableFailure,
        );
        outcome.next_retry_after_ms = None;

        assert!(
            RemoteNotificationAdapterSendRuntime::plan_send_result(
                RemoteNotificationAdapterSendInput {
                    envelope_report: report,
                    accepted_records: vec![source],
                    outcomes: vec![outcome],
                },
            )
            .is_err()
        );
    }

    fn delivered_outcome(dispatch_id: &str) -> RemoteNotificationAdapterSendOutcome {
        RemoteNotificationAdapterSendOutcome {
            dispatch_id: dispatch_id.to_owned(),
            status: RemoteNotificationAdapterSendStatus::Delivered,
            sent_at_ms: 300,
            next_retry_after_ms: None,
            adapter_message_id: Some("ws-message-1".to_owned()),
            error_summary: None,
            source_trace: "remote_notification_adapter_send:test".to_owned(),
        }
    }

    fn failure_outcome(
        dispatch_id: &str,
        status: RemoteNotificationAdapterSendStatus,
    ) -> RemoteNotificationAdapterSendOutcome {
        RemoteNotificationAdapterSendOutcome {
            dispatch_id: dispatch_id.to_owned(),
            status,
            sent_at_ms: 300,
            next_retry_after_ms: Some(500),
            adapter_message_id: None,
            error_summary: Some("adapter 返回可重试错误".to_owned()),
            source_trace: "remote_notification_adapter_send:test".to_owned(),
        }
    }

    fn accepted_record(dispatch_id: &str) -> RemoteNotificationOutboxRecord {
        RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
            dispatch_id: dispatch_id.to_owned(),
            plan: fanout_plan(dispatch_id),
            status: RemoteNotificationDispatchStatus::Attempting,
            recorded_at_ms: 100,
            updated_at_ms: 200,
            attempt_count: 0,
            next_retry_after_ms: None,
            last_error: None,
            adapter_message_id: None,
            source_trace: "remote_notification_adapter_send:test".to_owned(),
            reason: REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_PLANNED.to_owned(),
        })
        .expect("record")
    }

    fn batch() -> RemoteNotificationAdapterDispatchBatch {
        batch_with_id("dispatch-send")
    }

    fn batch_with_id(dispatch_id: &str) -> RemoteNotificationAdapterDispatchBatch {
        RemoteNotificationAdapterDispatchBatch {
            session_id: SessionId(71),
            transport: RemoteTransportKind::WebSocket,
            items: vec![RemoteNotificationAdapterDispatchItem {
                dispatch_id: dispatch_id.to_owned(),
                subscriber_id: "web-client".to_owned(),
                connection_id: "ws-conn".to_owned(),
                pairing_id: "pairing-local".to_owned(),
                delivery_count: 1,
                delivery_kinds: vec![RemoteNotificationKind::TaskStarted],
                attempt_count: 0,
            }],
            ack_records: Vec::new(),
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: Vec::new(),
        }
    }

    fn fanout_plan(_dispatch_id: &str) -> RemoteNotificationFanoutPlan {
        RemoteNotificationFanoutPlan {
            session_id: SessionId(71),
            subscriber_id: "web-client".to_owned(),
            connection_id: "ws-conn".to_owned(),
            pairing_id: "pairing-local".to_owned(),
            transport: RemoteTransportKind::WebSocket,
            subscribed_kinds: vec![RemoteNotificationKind::TaskStarted],
            delivered: vec![RemoteNotificationDelivery {
                kind: RemoteNotificationKind::TaskStarted,
                wire_event: remote_notification_wire_lifecycle_event(
                    SessionId(71),
                    "remote-send-test",
                    WireLifecycleStatus::Started,
                    "远控通知",
                    None,
                    100,
                ),
                bounded_body: "通知正文只包含 bounded summary".to_owned(),
                reference_ids: vec!["thread:71".to_owned()],
                local_action_hint: "在本机打开对应线程".to_owned(),
                severity: RemoteNotificationSeverity::Info,
            }],
            dropped_count: 0,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
        }
    }
}
