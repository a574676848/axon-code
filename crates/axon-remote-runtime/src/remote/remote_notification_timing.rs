use axon_domain::{AxonError, AxonResult};
use chrono::Timelike;

use crate::RemoteNotificationSeverity;

use super::{
    MINUTES_PER_DAY, REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS, RemoteNotificationDispatchStatus,
    RemoteNotificationOutboxAppendReport, RemoteNotificationOutboxRecord,
    RemoteNotificationOutboxRuntime,
};

/// durable 通知限流策略；达到窗口上限时只延后，不静默丢弃。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteNotificationThrottlePolicy {
    /// 统计窗口。
    pub window_ms: u64,
    /// 窗口内允许的活动记录数。
    pub max_active_records: usize,
}

/// 统一远控通知时序策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteNotificationTimingPolicy {
    /// 静默期开始分钟（本地时间）。
    pub quiet_start_minute: u16,
    /// 静默期结束分钟（本地时间）。
    pub quiet_end_minute: u16,
    /// 静默期内下一次重试的延后时长。
    pub quiet_retry_after_ms: u64,
    /// Info 通知合并窗口。
    pub info_coalesce_window_ms: u64,
    /// Info 通知限流窗口。
    pub info_throttle_window_ms: u64,
    /// Info 通知限流窗口内最大活动记录数。
    pub info_max_active_records: usize,
}

impl Default for RemoteNotificationTimingPolicy {
    fn default() -> Self {
        Self {
            quiet_start_minute: 22 * 60,
            quiet_end_minute: 7 * 60,
            quiet_retry_after_ms: 30 * 60 * 1_000,
            info_coalesce_window_ms: 1_000,
            info_throttle_window_ms: 1_000,
            info_max_active_records: 64,
        }
    }
}

impl RemoteNotificationTimingPolicy {
    /// 返回当前本地时间的分钟序号，避免各生产路径自行计算时区边界。
    #[must_use]
    pub fn local_minute_of_day() -> u16 {
        let now = chrono::Local::now();
        (now.hour() * 60 + now.minute()) as u16
    }
}

impl RemoteNotificationOutboxRuntime {
    /// 按统一 timing policy 写入远控出站队列。
    ///
    /// 只有主动通知（`Info`）参与静默期延期、合并与限流；`DirectReply` 是用户直答，
    /// 必须立即出站。Warning 与 ActionRequired 保持单条 durable 记录，避免重要告警
    /// 因合并而失真。
    pub fn append_timed_record(
        path: impl AsRef<std::path::Path>,
        record: &RemoteNotificationOutboxRecord,
        severity: RemoteNotificationSeverity,
        policy: RemoteNotificationTimingPolicy,
        now_minute: u16,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        validate_timing_policy(&policy, now_minute)?;
        if severity != RemoteNotificationSeverity::Info {
            return Self::append_record(path, record);
        }
        let candidate = if is_quiet_hours(&policy, now_minute) {
            let mut deferred = record.clone();
            deferred.status = RemoteNotificationDispatchStatus::Failed;
            deferred.next_retry_after_ms = Some(
                record
                    .updated_at_ms
                    .saturating_add(policy.quiet_retry_after_ms),
            );
            deferred.last_error = Some("notification quiet hours".to_owned());
            deferred.reason = format!("{}:quiet_hours", record.reason)
                .chars()
                .take(REMOTE_NOTIFICATION_OUTBOX_REASON_MAX_CHARS)
                .collect();
            deferred
        } else {
            record.clone()
        };
        Self::append_coalesced_rate_limited_record(
            path,
            &candidate,
            policy.info_coalesce_window_ms,
            RemoteNotificationThrottlePolicy {
                window_ms: policy.info_throttle_window_ms,
                max_active_records: policy.info_max_active_records,
            },
        )
    }
}

fn is_quiet_hours(policy: &RemoteNotificationTimingPolicy, now_minute: u16) -> bool {
    if now_minute >= MINUTES_PER_DAY {
        return false;
    }
    if policy.quiet_start_minute <= policy.quiet_end_minute {
        now_minute >= policy.quiet_start_minute && now_minute < policy.quiet_end_minute
    } else {
        now_minute >= policy.quiet_start_minute || now_minute < policy.quiet_end_minute
    }
}

fn validate_timing_policy(
    policy: &RemoteNotificationTimingPolicy,
    now_minute: u16,
) -> AxonResult<()> {
    if policy.quiet_start_minute >= MINUTES_PER_DAY
        || policy.quiet_end_minute >= MINUTES_PER_DAY
        || now_minute >= MINUTES_PER_DAY
        || policy.quiet_retry_after_ms == 0
        || policy.info_throttle_window_ms == 0
        || policy.info_max_active_records == 0
    {
        return Err(AxonError::runtime_failure("远控通知 timing policy 无效"));
    }
    Ok(())
}
