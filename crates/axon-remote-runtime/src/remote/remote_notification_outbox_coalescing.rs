//! 远控通知 outbox 的合并与限流写入策略。
//!
//! 合并键必须同时覆盖会话、订阅者、**连接（chat 维度）** 与原因：缺少连接维度会让
//! 不同会话的回复互相合并，被合并方既收不到回复，其 artifact 也会被调用方删除。
//!
//! 同时，只有真正合并到「他人」记录时才回填 `coalesced_dispatch_id`；合并到自身
//! dispatch 时回填会让调用方误删自己的 artifact，导致该条回复永远无法投递。

use std::path::Path;

use axon_domain::{AxonError, AxonResult};

use super::{
    REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS, RemoteNotificationDispatchStatus,
    RemoteNotificationOutboxAppendReport, RemoteNotificationOutboxRecord,
    RemoteNotificationOutboxRuntime, RemoteNotificationThrottlePolicy,
};

impl RemoteNotificationOutboxRuntime {
    /// 按订阅者限流写入 outbox；被限流记录保留为 pending 并设置 retry 时间。
    pub fn append_rate_limited_record(
        path: impl AsRef<Path>,
        record: &RemoteNotificationOutboxRecord,
        policy: RemoteNotificationThrottlePolicy,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        if policy.window_ms == 0 || policy.max_active_records == 0 {
            return Err(AxonError::runtime_failure("通知限流策略无效"));
        }
        let path = path.as_ref();
        let records = Self::read_records(path)?;
        let active = records
            .iter()
            .filter(|candidate| {
                candidate.session_id == record.session_id
                    && candidate.subscriber_id == record.subscriber_id
                    && matches!(
                        candidate.status,
                        RemoteNotificationDispatchStatus::Pending
                            | RemoteNotificationDispatchStatus::Attempting
                            | RemoteNotificationDispatchStatus::Failed
                    )
                    && record.updated_at_ms.saturating_sub(candidate.updated_at_ms)
                        <= policy.window_ms
            })
            .count();
        if active < policy.max_active_records {
            return Self::append_record(path, record);
        }
        let mut deferred = record.clone();
        deferred.status = RemoteNotificationDispatchStatus::Failed;
        deferred.next_retry_after_ms = Some(record.updated_at_ms.saturating_add(policy.window_ms));
        deferred.last_error = Some("notification rate limited".to_owned());
        deferred.reason = format!("{}:rate_limited", record.reason)
            .chars()
            .take(super::REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS)
            .collect();
        Self::append_record(path, &deferred)
    }

    /// 先合并重复通知，再对不同通知执行 durable 限流。
    pub fn append_coalesced_rate_limited_record(
        path: impl AsRef<Path>,
        record: &RemoteNotificationOutboxRecord,
        coalesce_window_ms: u64,
        policy: RemoteNotificationThrottlePolicy,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        let path_ref = path.as_ref();
        if coalesce_window_ms > 0 {
            let existing = Self::read_records(path_ref)?;
            if let Some(mut merged) = existing
                .into_iter()
                .find(|candidate| coalesces_with(candidate, record, coalesce_window_ms))
            {
                let merged_dispatch_id = merged.dispatch_id.clone();
                merge_into(&mut merged, record);
                merged.next_retry_after_ms =
                    max_retry_after(merged.next_retry_after_ms, record.next_retry_after_ms);
                let mut report = Self::append_record(path_ref, &merged)?;
                if merged_dispatch_id != record.dispatch_id {
                    report.coalesced_dispatch_id = Some(merged_dispatch_id);
                }
                return Ok(report);
            }
        }
        Self::append_rate_limited_record(path_ref, record, policy)
    }

    /// 在指定时间窗口内合并同一订阅者的重复通知，再按 LRU 规则持久化。
    pub fn append_coalesced_record(
        path: impl AsRef<Path>,
        record: &RemoteNotificationOutboxRecord,
        window_ms: u64,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        if window_ms == 0 {
            return Self::append_record(path, record);
        }
        let path = path.as_ref();
        let existing = Self::read_records(path)?;
        if let Some(mut merged) = existing
            .into_iter()
            .find(|candidate| coalesces_with(candidate, record, window_ms))
        {
            let merged_dispatch_id = merged.dispatch_id.clone();
            merge_into(&mut merged, record);
            let mut report = Self::append_record(path, &merged)?;
            if merged_dispatch_id != record.dispatch_id {
                report.coalesced_dispatch_id = Some(merged_dispatch_id);
            }
            return Ok(report);
        }
        Self::append_record(path, record)
    }
}

/// 判定既有记录是否可吸收当前记录：会话/订阅者/连接/原因四维相同，且仍在合并窗口内。
fn coalesces_with(
    candidate: &RemoteNotificationOutboxRecord,
    record: &RemoteNotificationOutboxRecord,
    window_ms: u64,
) -> bool {
    candidate.session_id == record.session_id
        && candidate.subscriber_id == record.subscriber_id
        && candidate.connection_id == record.connection_id
        && candidate.reason == record.reason
        && matches!(
            candidate.status,
            RemoteNotificationDispatchStatus::Pending
                | RemoteNotificationDispatchStatus::Attempting
                | RemoteNotificationDispatchStatus::Failed
        )
        && record.updated_at_ms.saturating_sub(candidate.updated_at_ms) <= window_ms
}

fn merge_into(
    merged: &mut RemoteNotificationOutboxRecord,
    record: &RemoteNotificationOutboxRecord,
) {
    merged.delivery_count = merged.delivery_count.saturating_add(record.delivery_count);
    merged.dropped_count = merged.dropped_count.saturating_add(record.dropped_count);
    merged
        .delivery_kinds
        .extend(record.delivery_kinds.iter().copied());
    if merged.delivery_kinds.len() > REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS {
        let overflow = merged
            .delivery_kinds
            .len()
            .saturating_sub(REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS);
        merged
            .delivery_kinds
            .truncate(REMOTE_PROTOCOL_NOTIFICATION_BATCH_MAX_ITEMS);
        merged.dropped_count = merged.dropped_count.saturating_add(overflow);
    }
    merged.updated_at_ms = merged.updated_at_ms.max(record.updated_at_ms);
}

fn max_retry_after(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
#[path = "remote_notification_outbox_coalescing_tests.rs"]
mod tests;
