use super::{
    REMOTE_NOTIFICATION_OUTBOX_ADAPTER_MESSAGE_ID_MAX_CHARS,
    REMOTE_NOTIFICATION_OUTBOX_DISPATCH_ID_MAX_CHARS, REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS,
    REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS, REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS,
    REMOTE_NOTIFICATION_OUTBOX_SCHEMA_VERSION, REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS,
    REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS, REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS,
    REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS, REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS,
    RemoteNotificationDispatchStatus, RemoteNotificationFanoutPlan, RemoteNotificationKind,
    RemoteNotificationOutboxInput, RemoteNotificationOutboxRecord,
};
use axon_domain::{AxonError, AxonResult};

pub(super) struct RemoteNotificationOutboxCommonValidation<'a> {
    pub(super) status: RemoteNotificationDispatchStatus,
    pub(super) session_id: u64,
    pub(super) delivery_count: usize,
    pub(super) dropped_count: usize,
    pub(super) delivery_kinds: &'a [RemoteNotificationKind],
    pub(super) recorded_at_ms: u64,
    pub(super) updated_at_ms: u64,
    pub(super) attempt_count: u32,
    pub(super) next_retry_after_ms: Option<u64>,
    pub(super) last_error: Option<&'a str>,
    pub(super) adapter_message_id: Option<&'a str>,
    pub(super) attempt_started_at_ms: Option<u64>,
    pub(super) attempt_owner: Option<&'a str>,
    pub(super) source_trace: &'a str,
    pub(super) reason: &'a str,
}

pub(super) fn validate_common(
    common: &RemoteNotificationOutboxCommonValidation<'_>,
) -> AxonResult<()> {
    if common.session_id == 0
        || common.recorded_at_ms == 0
        || common.updated_at_ms < common.recorded_at_ms
    {
        return Err(AxonError::runtime_failure(
            "远控通知 outbox 时间或 session 无效",
        ));
    }
    if common.delivery_count > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS
        || common.dropped_count > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS
        || common.delivery_kinds.len() != common.delivery_count
    {
        return Err(AxonError::runtime_failure("远控通知 outbox 投递数量无效"));
    }
    if common.status != RemoteNotificationDispatchStatus::Abandoned && common.delivery_count == 0 {
        return Err(AxonError::runtime_failure(
            "远控通知 outbox 需要至少一个投递项",
        ));
    }
    if common.attempt_count > REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS {
        return Err(AxonError::runtime_failure("远控通知 outbox 尝试次数无效"));
    }
    if let Some(retry_at) = common.next_retry_after_ms
        && (common.status != RemoteNotificationDispatchStatus::Failed
            || retry_at <= common.updated_at_ms)
    {
        return Err(AxonError::runtime_failure("远控通知 outbox 重试时间无效"));
    }
    if let Some(error) = common.last_error {
        validate_bounded(
            error,
            REMOTE_NOTIFICATION_OUTBOX_ERROR_MAX_CHARS,
            "远控通知 outbox 错误摘要无效",
        )?;
    }
    if let Some(message_id) = common.adapter_message_id {
        validate_bounded(
            message_id,
            REMOTE_NOTIFICATION_OUTBOX_ADAPTER_MESSAGE_ID_MAX_CHARS,
            "远控通知 outbox adapter message id 无效",
        )?;
    }
    if common.status == RemoteNotificationDispatchStatus::Attempting
        && (common.attempt_started_at_ms.is_none() || common.attempt_owner.is_none())
    {
        return Err(AxonError::runtime_failure(
            "远控通知 attempting 缺少 owner 或开始时间",
        ));
    }
    if let Some(owner) = common.attempt_owner {
        validate_bounded(
            owner,
            REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS,
            "远控通知 attempting owner 无效",
        )?;
    }
    validate_bounded(
        common.source_trace,
        REMOTE_NOTIFICATION_OUTBOX_SOURCE_TRACE_MAX_CHARS,
        "远控通知 outbox source trace 无效",
    )?;
    validate_bounded(
        common.reason,
        REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS,
        "远控通知 outbox 原因无效",
    )
}

pub(super) fn validate_bounded(
    value: &str,
    max_chars: usize,
    message: &'static str,
) -> AxonResult<()> {
    if value.trim().is_empty() || value.chars().count() > max_chars {
        return Err(AxonError::runtime_failure(message));
    }
    Ok(())
}

pub(super) fn is_pending(record: &RemoteNotificationOutboxRecord, now_ms: u64) -> bool {
    match record.status {
        RemoteNotificationDispatchStatus::Pending => true,
        RemoteNotificationDispatchStatus::Failed => record
            .next_retry_after_ms
            .is_some_and(|retry_at| retry_at <= now_ms),
        RemoteNotificationDispatchStatus::Attempting
        | RemoteNotificationDispatchStatus::Delivered
        | RemoteNotificationDispatchStatus::Abandoned => false,
    }
}

pub(super) fn validate_input(input: &RemoteNotificationOutboxInput) -> AxonResult<()> {
    validate_bounded(
        &input.dispatch_id,
        REMOTE_NOTIFICATION_OUTBOX_DISPATCH_ID_MAX_CHARS,
        "远控通知 outbox dispatch id 无效",
    )?;
    validate_plan(&input.plan)?;
    let delivery_kinds = input
        .plan
        .delivered
        .iter()
        .map(|delivery| delivery.kind)
        .collect::<Vec<_>>();
    validate_common(&RemoteNotificationOutboxCommonValidation {
        status: input.status,
        session_id: input.plan.session_id.0,
        delivery_count: input.plan.delivered.len(),
        dropped_count: input.plan.dropped_count,
        delivery_kinds: &delivery_kinds,
        recorded_at_ms: input.recorded_at_ms,
        updated_at_ms: input.updated_at_ms,
        attempt_count: input.attempt_count,
        next_retry_after_ms: input.next_retry_after_ms,
        last_error: input.last_error.as_deref(),
        adapter_message_id: None,
        attempt_started_at_ms: (input.status == RemoteNotificationDispatchStatus::Attempting)
            .then_some(input.updated_at_ms),
        attempt_owner: (input.status == RemoteNotificationDispatchStatus::Attempting)
            .then_some(input.source_trace.as_str()),
        source_trace: &input.source_trace,
        reason: &input.reason,
    })
}

pub(super) fn validate_record(record: &RemoteNotificationOutboxRecord) -> AxonResult<()> {
    if record.schema_version != REMOTE_NOTIFICATION_OUTBOX_SCHEMA_VERSION {
        return Err(AxonError::runtime_failure(
            "远控通知 outbox schema 版本无效",
        ));
    }
    if !record.local_only
        || record.network_send_allowed
        || record.raw_payload_forward_allowed
        || record.raw_body_persisted
        || record.raw_payload_persisted
    {
        return Err(AxonError::runtime_failure(
            "远控通知 outbox 不能开放网络、raw payload",
        ));
    }
    validate_bounded(
        &record.dispatch_id,
        REMOTE_NOTIFICATION_OUTBOX_DISPATCH_ID_MAX_CHARS,
        "远控通知 outbox dispatch id 无效",
    )?;
    validate_bounded(
        &record.subscriber_id,
        REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS,
        "远控通知 outbox subscriber id 无效",
    )?;
    validate_bounded(
        &record.connection_id,
        REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS,
        "远控通知 outbox connection id 无效",
    )?;
    validate_bounded(
        &record.pairing_id,
        REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS,
        "远控通知 outbox pairing id 无效",
    )?;
    validate_common(&RemoteNotificationOutboxCommonValidation {
        status: record.status,
        session_id: record.session_id,
        delivery_count: record.delivery_count,
        dropped_count: record.dropped_count,
        delivery_kinds: &record.delivery_kinds,
        recorded_at_ms: record.recorded_at_ms,
        updated_at_ms: record.updated_at_ms,
        attempt_count: record.attempt_count,
        next_retry_after_ms: record.next_retry_after_ms,
        last_error: record.last_error.as_deref(),
        adapter_message_id: record.adapter_message_id.as_deref(),
        attempt_started_at_ms: record.attempt_started_at_ms,
        attempt_owner: record.attempt_owner.as_deref(),
        source_trace: &record.source_trace,
        reason: &record.reason,
    })
}

fn validate_plan(plan: &RemoteNotificationFanoutPlan) -> AxonResult<()> {
    if !plan.local_only || plan.network_send_allowed || plan.raw_payload_forward_allowed {
        return Err(AxonError::runtime_failure(
            "远控通知 fanout plan 不能进入 unsafe outbox",
        ));
    }
    validate_bounded(
        &plan.subscriber_id,
        REMOTE_PROTOCOL_CLIENT_ID_MAX_CHARS,
        "远控通知 outbox subscriber id 无效",
    )?;
    validate_bounded(
        &plan.connection_id,
        REMOTE_TRANSPORT_CONNECTION_ID_MAX_CHARS,
        "远控通知 outbox connection id 无效",
    )?;
    validate_bounded(
        &plan.pairing_id,
        REMOTE_TRANSPORT_PAIRING_ID_MAX_CHARS,
        "远控通知 outbox pairing id 无效",
    )
}
