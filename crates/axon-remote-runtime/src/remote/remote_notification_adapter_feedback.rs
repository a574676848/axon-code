//! 远控通知 adapter feedback 计划。
//!
//! 该模块只把 adapter 发送反馈转换为 outbox 状态记录，不执行网络发送。

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS, REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS,
    REMOTE_NOTIFICATION_OUTBOX_REASON_NETWORK_CLOSED,
    REMOTE_NOTIFICATION_OUTBOX_REASON_RAW_PAYLOAD_CLOSED,
    REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED,
    REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS, RemoteNotificationDispatchStatus,
    RemoteNotificationOutboxRecord, RemoteTransportKind,
};

/// 远控通知 adapter feedback 可重试失败原因。
pub const REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_RETRYABLE: &str =
    "remote_notification_adapter_feedback:retryable";
/// 远控通知 adapter feedback 永久失败原因。
pub const REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_PERMANENT: &str =
    "remote_notification_adapter_feedback:permanent";
/// 远控通知 adapter feedback 已规划原因。
pub const REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_PLANNED: &str =
    "remote_notification_adapter_feedback:planned";

/// 远控通知 adapter feedback 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteNotificationAdapterFeedbackStatus {
    /// adapter 发送失败但允许稍后重试。
    RetryableFailure,
    /// adapter 发送失败且不能重试。
    PermanentFailure,
}

/// 远控通知 adapter feedback 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterFeedbackInput {
    /// dispatch id。
    pub dispatch_id: String,
    /// 目标会话。
    pub session_id: SessionId,
    /// 传输类型。
    pub transport: RemoteTransportKind,
    /// 反馈状态。
    pub status: RemoteNotificationAdapterFeedbackStatus,
    /// 反馈发生时间。
    pub feedback_at_ms: u64,
    /// 下次可重试时间。
    pub next_retry_after_ms: Option<u64>,
    /// 有界错误摘要。
    pub last_error: String,
    /// source trace。
    pub source_trace: String,
}

/// 远控通知 adapter feedback 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteNotificationAdapterFeedbackReport {
    /// 应追加到 outbox 的状态记录。
    pub record: RemoteNotificationOutboxRecord,
    /// 是否只允许本地 adapter 消费。
    pub local_only: bool,
    /// 是否允许主动网络发送。
    pub network_send_allowed: bool,
    /// 是否允许转发 raw payload。
    pub raw_payload_forward_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 远控通知 adapter feedback runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteNotificationAdapterFeedbackRuntime;

impl RemoteNotificationAdapterFeedbackRuntime {
    /// 基于已有 outbox 记录规划 adapter 反馈状态。
    ///
    /// # Errors
    ///
    /// 当 dispatch/session/transport 不匹配、时间或重试策略无效、错误摘要越界时返回错误。
    pub fn plan_feedback(
        source: &RemoteNotificationOutboxRecord,
        input: RemoteNotificationAdapterFeedbackInput,
    ) -> AxonResult<RemoteNotificationAdapterFeedbackReport> {
        validate_match(source, &input)?;
        validate_input(source, &input)?;
        let mut record = source.clone();
        record.status = dispatch_status(input.status);
        record.updated_at_ms = input.feedback_at_ms;
        record.attempt_count = source.attempt_count + 1;
        record.next_retry_after_ms = input.next_retry_after_ms;
        record.last_error = Some(input.last_error);
        record.adapter_message_id = None;
        record.source_trace = input.source_trace;
        feedback_reason(input.status).clone_into(&mut record.reason);
        record.local_only = true;
        record.network_send_allowed = false;
        record.raw_payload_forward_allowed = false;
        record.raw_body_persisted = false;
        record.raw_payload_persisted = false;
        Ok(RemoteNotificationAdapterFeedbackReport {
            record,
            local_only: true,
            network_send_allowed: false,
            raw_payload_forward_allowed: false,
            reasons: feedback_reasons(input.status),
        })
    }
}

fn validate_match(
    source: &RemoteNotificationOutboxRecord,
    input: &RemoteNotificationAdapterFeedbackInput,
) -> AxonResult<()> {
    if source.dispatch_id != input.dispatch_id
        || source.session_id != input.session_id.0
        || source.transport != input.transport
    {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter feedback 与 outbox 记录不匹配",
        ));
    }
    Ok(())
}

fn validate_input(
    source: &RemoteNotificationOutboxRecord,
    input: &RemoteNotificationAdapterFeedbackInput,
) -> AxonResult<()> {
    if input.session_id.0 == 0 || input.feedback_at_ms <= source.updated_at_ms {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter feedback 时间或 session 无效",
        ));
    }
    if source.attempt_count >= REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS {
        return Err(AxonError::runtime_failure(
            "远控通知 adapter feedback 尝试次数已耗尽",
        ));
    }
    validate_retry_policy(input)?;
    validate_bounded(
        &input.last_error,
        REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS,
        "远控通知 adapter feedback 错误摘要无效",
    )?;
    validate_bounded(
        &input.source_trace,
        REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS,
        "远控通知 adapter feedback source trace 无效",
    )
}

fn validate_retry_policy(input: &RemoteNotificationAdapterFeedbackInput) -> AxonResult<()> {
    match (input.status, input.next_retry_after_ms) {
        (RemoteNotificationAdapterFeedbackStatus::RetryableFailure, Some(retry_at))
            if retry_at > input.feedback_at_ms =>
        {
            Ok(())
        }
        (RemoteNotificationAdapterFeedbackStatus::PermanentFailure, None) => Ok(()),
        _ => Err(AxonError::runtime_failure(
            "远控通知 adapter feedback 重试策略无效",
        )),
    }
}

fn validate_bounded(value: &str, max_chars: usize, message: &'static str) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

fn dispatch_status(
    status: RemoteNotificationAdapterFeedbackStatus,
) -> RemoteNotificationDispatchStatus {
    match status {
        RemoteNotificationAdapterFeedbackStatus::RetryableFailure => {
            RemoteNotificationDispatchStatus::Failed
        }
        RemoteNotificationAdapterFeedbackStatus::PermanentFailure => {
            RemoteNotificationDispatchStatus::Abandoned
        }
    }
}

fn feedback_reason(status: RemoteNotificationAdapterFeedbackStatus) -> &'static str {
    match status {
        RemoteNotificationAdapterFeedbackStatus::RetryableFailure => {
            REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_RETRYABLE
        }
        RemoteNotificationAdapterFeedbackStatus::PermanentFailure => {
            REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_PERMANENT
        }
    }
}

fn feedback_reasons(status: RemoteNotificationAdapterFeedbackStatus) -> Vec<String> {
    vec![
        REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_PLANNED.to_owned(),
        feedback_reason(status).to_owned(),
        REMOTE_NOTIFICATION_OUTBOX_REASON_STATUS_FOLDED.to_owned(),
        REMOTE_NOTIFICATION_OUTBOX_REASON_NETWORK_CLOSED.to_owned(),
        REMOTE_NOTIFICATION_OUTBOX_REASON_RAW_PAYLOAD_CLOSED.to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use axon_domain::SessionId;

    use crate::{
        RemoteNotificationDelivery, RemoteNotificationFanoutPlan, RemoteNotificationKind,
        RemoteNotificationOutboxInput, RemoteNotificationSeverity, RemoteTransportKind,
        WireLifecycleStatus, remote_notification_wire_lifecycle_event,
    };

    use super::*;

    #[test]
    fn adapter_feedback_plans_retryable_record() {
        let source = outbox_record("dispatch-feedback", 0);
        let report = RemoteNotificationAdapterFeedbackRuntime::plan_feedback(
            &source,
            feedback_input(
                "dispatch-feedback",
                RemoteNotificationAdapterFeedbackStatus::RetryableFailure,
                Some(500),
            ),
        )
        .expect("feedback");

        assert_eq!(
            report.record.status,
            RemoteNotificationDispatchStatus::Failed
        );
        assert_eq!(report.record.attempt_count, 1);
        assert_eq!(report.record.next_retry_after_ms, Some(500));
        assert!(report.local_only);
        assert!(!report.network_send_allowed);
        assert!(!report.raw_payload_forward_allowed);
    }

    #[test]
    fn adapter_feedback_rejects_invalid_retry_policy() {
        let source = outbox_record("dispatch-feedback-invalid", 0);

        assert!(
            RemoteNotificationAdapterFeedbackRuntime::plan_feedback(
                &source,
                feedback_input(
                    "dispatch-feedback-invalid",
                    RemoteNotificationAdapterFeedbackStatus::RetryableFailure,
                    None,
                ),
            )
            .is_err()
        );
    }

    fn feedback_input(
        dispatch_id: &str,
        status: RemoteNotificationAdapterFeedbackStatus,
        next_retry_after_ms: Option<u64>,
    ) -> RemoteNotificationAdapterFeedbackInput {
        RemoteNotificationAdapterFeedbackInput {
            dispatch_id: dispatch_id.to_owned(),
            session_id: SessionId(51),
            transport: RemoteTransportKind::WebSocket,
            status,
            feedback_at_ms: 300,
            next_retry_after_ms,
            last_error: "adapter 返回可重试错误".to_owned(),
            source_trace: "remote_notification_adapter_feedback:test".to_owned(),
        }
    }

    fn outbox_record(dispatch_id: &str, attempt_count: u32) -> RemoteNotificationOutboxRecord {
        RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
            dispatch_id: dispatch_id.to_owned(),
            plan: fanout_plan(),
            status: RemoteNotificationDispatchStatus::Attempting,
            recorded_at_ms: 100,
            updated_at_ms: 200,
            attempt_count,
            next_retry_after_ms: None,
            last_error: None,
            adapter_message_id: None,
            source_trace: "remote_notification_adapter_feedback:test".to_owned(),
            reason: REMOTE_NOTIFICATION_ADAPTER_FEEDBACK_REASON_PLANNED.to_owned(),
        })
        .expect("record")
    }

    fn fanout_plan() -> RemoteNotificationFanoutPlan {
        RemoteNotificationFanoutPlan {
            session_id: SessionId(51),
            subscriber_id: "web-client".to_owned(),
            connection_id: "ws-conn".to_owned(),
            pairing_id: "pairing-local".to_owned(),
            transport: RemoteTransportKind::WebSocket,
            subscribed_kinds: vec![RemoteNotificationKind::TaskStarted],
            delivered: vec![RemoteNotificationDelivery {
                kind: RemoteNotificationKind::TaskStarted,
                wire_event: remote_notification_wire_lifecycle_event(
                    SessionId(51),
                    "remote-feedback-test",
                    WireLifecycleStatus::Started,
                    "远控通知",
                    None,
                    100,
                ),
                bounded_body: "通知正文只包含 bounded summary".to_owned(),
                reference_ids: vec!["thread:51".to_owned()],
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
