//! 远控通知 adapter send 规划辅助逻辑。

use std::collections::HashMap;

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::clamp_text_keeping_edges;
use crate::{
    REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS, REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS,
    REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED,
    REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS, RemoteNotificationAdapterEnvelope,
    RemoteNotificationAdapterEnvelopeReport, RemoteNotificationAdapterFeedbackInput,
    RemoteNotificationAdapterFeedbackRuntime, RemoteNotificationAdapterFeedbackStatus,
    RemoteNotificationDispatchStatus, RemoteNotificationOutboxRecord,
};

use super::{
    REMOTE_NOTIFICATION_ADAPTER_SEND_MESSAGE_ID_MAX_CHARS,
    REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_DELIVERED,
    REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_NETWORK_CLOSED,
    REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_PLANNED,
    REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_RAW_PAYLOAD_CLOSED,
    RemoteNotificationAdapterSendOutcome, RemoteNotificationAdapterSendReceipt,
    RemoteNotificationAdapterSendReport, RemoteNotificationAdapterSendStatus,
};

pub(super) fn validate_report(report: &RemoteNotificationAdapterEnvelopeReport) -> AxonResult<()> {
    if report.session_id.0 == 0
        || !report.local_only
        || report.network_send_allowed
        || report.raw_payload_forward_allowed
    {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter send envelope 安全边界无效",
        ));
    }
    Ok(())
}

pub(super) fn accepted_by_dispatch(
    records: Vec<RemoteNotificationOutboxRecord>,
) -> AxonResult<HashMap<String, RemoteNotificationOutboxRecord>> {
    let mut accepted = HashMap::with_capacity(records.len());
    for record in records {
        if record.status != RemoteNotificationDispatchStatus::Attempting {
            return Err(AxonError::runtime_failure(
                "远控通知 adapter send 只接受 Attempting 记录",
            ));
        }
        if accepted
            .insert(record.dispatch_id.clone(), record)
            .is_some()
        {
            return Err(AxonError::runtime_failure(
                "远控通知 adapter send accepted record 重复",
            ));
        }
    }
    Ok(accepted)
}

pub(super) fn outcomes_by_dispatch(
    outcomes: Vec<RemoteNotificationAdapterSendOutcome>,
) -> AxonResult<HashMap<String, RemoteNotificationAdapterSendOutcome>> {
    let mut mapped = HashMap::with_capacity(outcomes.len());
    for outcome in outcomes {
        validate_outcome(&outcome)?;
        if mapped
            .insert(outcome.dispatch_id.clone(), outcome)
            .is_some()
        {
            return Err(AxonError::runtime_failure(
                "远控通知 adapter send outcome 重复",
            ));
        }
    }
    Ok(mapped)
}

pub(super) fn validate_match(
    report: &RemoteNotificationAdapterEnvelopeReport,
    envelope: &RemoteNotificationAdapterEnvelope,
    source: &RemoteNotificationOutboxRecord,
    outcome: &RemoteNotificationAdapterSendOutcome,
) -> AxonResult<()> {
    if envelope.session_id != report.session_id
        || envelope.transport != report.transport
        || source.session_id != report.session_id.0
        || source.transport != report.transport
        || source.dispatch_id != envelope.dispatch_id
        || source.delivery_count != envelope.delivery_count
        || source.delivery_kinds != envelope.delivery_kinds
        || outcome.dispatch_id != envelope.dispatch_id
        || outcome.sent_at_ms <= source.updated_at_ms
    {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter send envelope、record 或 outcome 不匹配",
        ));
    }
    Ok(())
}

pub(super) fn outcome_record(
    source: &RemoteNotificationOutboxRecord,
    outcome: &RemoteNotificationAdapterSendOutcome,
) -> AxonResult<RemoteNotificationOutboxRecord> {
    if source.attempt_count >= REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter send 尝试次数已耗尽",
        ));
    }
    match outcome.status {
        RemoteNotificationAdapterSendStatus::Delivered => Ok(delivered_record(source, outcome)),
        RemoteNotificationAdapterSendStatus::RetryableFailure => failure_record(
            source,
            outcome,
            RemoteNotificationAdapterFeedbackStatus::RetryableFailure,
        ),
        RemoteNotificationAdapterSendStatus::PermanentFailure => failure_record(
            source,
            outcome,
            RemoteNotificationAdapterFeedbackStatus::PermanentFailure,
        ),
    }
}

pub(super) fn receipt(
    outcome: &RemoteNotificationAdapterSendOutcome,
    record: &RemoteNotificationOutboxRecord,
) -> RemoteNotificationAdapterSendReceipt {
    RemoteNotificationAdapterSendReceipt {
        dispatch_id: outcome.dispatch_id.clone(),
        status: outcome.status,
        record_status: record.status,
        adapter_message_id: outcome.adapter_message_id.clone(),
    }
}

pub(super) fn send_report(
    envelope_report: &RemoteNotificationAdapterEnvelopeReport,
    records: Vec<RemoteNotificationOutboxRecord>,
    receipts: Vec<RemoteNotificationAdapterSendReceipt>,
) -> RemoteNotificationAdapterSendReport {
    RemoteNotificationAdapterSendReport {
        session_id: envelope_report.session_id,
        transport: envelope_report.transport,
        delivered_count: receipts
            .iter()
            .filter(|receipt| receipt.status == RemoteNotificationAdapterSendStatus::Delivered)
            .count(),
        retryable_failure_count: receipts
            .iter()
            .filter(|receipt| {
                receipt.status == RemoteNotificationAdapterSendStatus::RetryableFailure
            })
            .count(),
        permanent_failure_count: receipts
            .iter()
            .filter(|receipt| {
                receipt.status == RemoteNotificationAdapterSendStatus::PermanentFailure
            })
            .count(),
        records,
        receipts,
        local_only: true,
        network_send_allowed: false,
        raw_payload_forward_allowed: false,
        reasons: vec![
            REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_PLANNED.to_owned(),
            REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED.to_owned(),
            REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_NETWORK_CLOSED.to_owned(),
            REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
        ],
    }
}

fn validate_outcome(outcome: &RemoteNotificationAdapterSendOutcome) -> AxonResult<()> {
    validate_bounded(
        &outcome.dispatch_id,
        crate::REMOTE_NOTIFICATION_OUTBOX_DISPATCH_ID_MAX_CHARS,
        "远控通知 adapter send dispatch id 无效",
    )?;
    validate_bounded(
        &outcome.source_trace,
        REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS,
        "远控通知 adapter send source trace 无效",
    )?;
    if let Some(message_id) = outcome.adapter_message_id.as_deref() {
        validate_bounded(
            message_id,
            REMOTE_NOTIFICATION_ADAPTER_SEND_MESSAGE_ID_MAX_CHARS,
            "远控通知 adapter send message id 无效",
        )?;
    }
    validate_outcome_policy(outcome)
}

fn validate_outcome_policy(outcome: &RemoteNotificationAdapterSendOutcome) -> AxonResult<()> {
    match outcome.status {
        RemoteNotificationAdapterSendStatus::Delivered => {
            if outcome.next_retry_after_ms.is_some() || outcome.error_summary.is_some() {
                return Err(AxonError::runtime_failure(
                    "远控通知 adapter send 投递成功不能携带 retry 或错误",
                ));
            }
            Ok(())
        }
        RemoteNotificationAdapterSendStatus::RetryableFailure => {
            let retry_at = outcome.next_retry_after_ms.ok_or({
                AxonError::runtime_failure("远控通知 adapter send 缺少 retry 时间")
            })?;
            if retry_at <= outcome.sent_at_ms {
                return Err(AxonError::runtime_failure(
                    "远控通知 adapter send retry 时间无效",
                ));
            }
            validate_required_error(outcome)
        }
        RemoteNotificationAdapterSendStatus::PermanentFailure => {
            if outcome.next_retry_after_ms.is_some() {
                return Err(AxonError::runtime_failure(
                    "远控通知 adapter send 永久失败不能携带 retry",
                ));
            }
            validate_required_error(outcome)
        }
    }
}

/// 收敛 adapter 错误摘要，使其满足本层的长度契约。
///
/// 摘要来源是外部供应商的错误体，长度不受我们控制；直接送验会让一条过长的错误串把
/// 「网络确实失败了」判成「outcome 无效」，于是这条失败既记不下来、也不会进入重试。
/// 因此这里统一按上限收敛（保留头尾、中间省略），校验只针对收敛后的值。
fn bounded_error_summary(outcome: &RemoteNotificationAdapterSendOutcome) -> Option<String> {
    outcome
        .error_summary
        .as_deref()
        .map(|error| clamp_text_keeping_edges(error, REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS))
}

fn validate_required_error(outcome: &RemoteNotificationAdapterSendOutcome) -> AxonResult<()> {
    let error = bounded_error_summary(outcome).ok_or(AxonError::runtime_failure(
        "远控通知 adapter send 缺少错误摘要",
    ))?;
    validate_bounded(
        &error,
        REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS,
        "远控通知 adapter send 错误摘要无效",
    )
}

fn delivered_record(
    source: &RemoteNotificationOutboxRecord,
    outcome: &RemoteNotificationAdapterSendOutcome,
) -> RemoteNotificationOutboxRecord {
    let mut record = source.clone();
    record.status = RemoteNotificationDispatchStatus::Delivered;
    record.updated_at_ms = outcome.sent_at_ms;
    record.attempt_count = source.attempt_count + 1;
    record.next_retry_after_ms = None;
    record.last_error = None;
    record.attempt_started_at_ms = None;
    record.attempt_owner = None;
    record.recovery_marker = None;
    record
        .adapter_message_id
        .clone_from(&outcome.adapter_message_id);
    record.source_trace.clone_from(&outcome.source_trace);
    REMOTE_NOTIFICATION_ADAPTER_SEND_REASON_DELIVERED.clone_into(&mut record.reason);
    record
}

fn failure_record(
    source: &RemoteNotificationOutboxRecord,
    outcome: &RemoteNotificationAdapterSendOutcome,
    status: RemoteNotificationAdapterFeedbackStatus,
) -> AxonResult<RemoteNotificationOutboxRecord> {
    let mut record = RemoteNotificationAdapterFeedbackRuntime::plan_feedback(
        source,
        RemoteNotificationAdapterFeedbackInput {
            dispatch_id: outcome.dispatch_id.clone(),
            session_id: SessionId(source.session_id),
            transport: source.transport,
            status,
            feedback_at_ms: outcome.sent_at_ms,
            next_retry_after_ms: outcome.next_retry_after_ms,
            last_error: bounded_error_summary(outcome).unwrap_or_default(),
            source_trace: outcome.source_trace.clone(),
        },
    )?
    .record;
    record.attempt_started_at_ms = None;
    record.attempt_owner = None;
    Ok(record)
}

fn validate_bounded(value: &str, max_chars: usize, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}
