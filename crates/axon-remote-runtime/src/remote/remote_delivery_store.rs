//! 渠道无关的 durable 投递存储门面。
//!
//! `DeliveryStore` 只管理投递记录生命周期，不接触 Task/Execution lease。

use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult};
use axon_domain::{DeliveryHealth, HealthBacklog, HealthStatusCounts};

use crate::{
    REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS, RemoteNotificationDispatchStatus,
    RemoteNotificationOutboxRecord, RemoteNotificationOutboxRuntime, RemoteTransportKind,
    remote_notification_outbox::RemoteNotificationOutboxAppendReport,
};

const DELIVERY_STORE_STATE_NAMESPACE: &str = "remote_notification_outbox";
const DELIVERY_STORE_STATUS_PENDING: &str = "pending";
const DELIVERY_STORE_STATUS_ATTEMPTING: &str = "attempting";
const DELIVERY_STORE_STATUS_FAILED: &str = "failed";
/// 未完成投递状态：仍需要继续尝试或等待重试。
const UNFINISHED_STATUSES: [&str; 3] = [
    DELIVERY_STORE_STATUS_PENDING,
    DELIVERY_STORE_STATUS_ATTEMPTING,
    DELIVERY_STORE_STATUS_FAILED,
];

/// durable 投递存储；慢渠道在此处排队和重试，不持有执行租约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryStore {
    path: PathBuf,
}

impl DeliveryStore {
    /// 创建绑定到指定 SQLite 文件的投递存储。
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// 返回底层存储路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 聚合投递积压指标：按状态计数并给出最老积压记录距今的时长。
    pub fn summary(&self, now_ms: u64) -> AxonResult<DeliveryHealth> {
        let summaries = crate::RemoteStateStoreRuntime::status_summary(
            &self.path,
            DELIVERY_STORE_STATE_NAMESPACE,
        )?;
        let mut pending = 0_u64;
        let mut oldest_updated_at_ms: Option<u64> = None;
        for summary in &summaries {
            if !UNFINISHED_STATUSES.contains(&summary.status.as_str()) {
                continue;
            }
            pending = pending.saturating_add(summary.count);
            oldest_updated_at_ms = Some(match oldest_updated_at_ms {
                Some(current) => current.min(summary.oldest_updated_at_ms),
                None => summary.oldest_updated_at_ms,
            });
        }
        let counts = HealthStatusCounts::from_statuses(
            summaries
                .into_iter()
                .map(|summary| (summary.status, summary.count)),
        );
        Ok(DeliveryHealth {
            counts,
            backlog: HealthBacklog {
                pending,
                oldest_age_ms: oldest_updated_at_ms.map(|oldest| now_ms.saturating_sub(oldest)),
            },
        })
    }

    /// 追加一条投递记录；相同 dispatch id 会更新当前状态。
    pub fn append(
        &self,
        record: &RemoteNotificationOutboxRecord,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        RemoteNotificationOutboxRuntime::append_record(&self.path, record)
    }

    /// 在窗口内合并重复通知后追加，避免重复投递。
    pub fn append_deduplicated(
        &self,
        record: &RemoteNotificationOutboxRecord,
        window_ms: u64,
    ) -> AxonResult<RemoteNotificationOutboxAppendReport> {
        RemoteNotificationOutboxRuntime::append_coalesced_record(&self.path, record, window_ms)
    }

    /// 有界领取待投递记录并原子写入 Attempting 检查点。
    pub fn claim(
        &self,
        transport: RemoteTransportKind,
        now_ms: u64,
        limit: usize,
        owner: &str,
    ) -> AxonResult<Vec<RemoteNotificationOutboxRecord>> {
        if now_ms == 0 || limit == 0 || owner.trim().is_empty() {
            return Err(AxonError::runtime_failure("DeliveryStore claim 参数无效"));
        }
        let batch_size = limit.max(32);
        let max_scan_records = 256;
        let mut offset = 0;
        let mut claimed = Vec::new();

        while claimed.len() < limit && offset < max_scan_records {
            let chunk_limit = batch_size.min(max_scan_records - offset);
            let candidates = crate::RemoteStateStoreRuntime::read_indexed_paged::<
                RemoteNotificationOutboxRecord,
            >(
                &self.path,
                DELIVERY_STORE_STATE_NAMESPACE,
                None,
                &[DELIVERY_STORE_STATUS_PENDING, DELIVERY_STORE_STATUS_FAILED],
                offset,
                chunk_limit,
            )?;
            if candidates.is_empty() {
                break;
            }
            let read_count = candidates.len();
            offset = offset.saturating_add(read_count);

            for record in candidates {
                if record.transport == transport
                    && record.attempt_count < REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS
                    && is_pending(&record, now_ms)
                {
                    let attempting = RemoteNotificationOutboxRuntime::mark_attempting(
                        &self.path,
                        &record.dispatch_id,
                        now_ms,
                        owner,
                    )?;
                    claimed.push(attempting);
                    if claimed.len() >= limit {
                        break;
                    }
                }
            }
            if read_count < chunk_limit {
                break;
            }
        }
        Ok(claimed)
    }

    /// 持久化一次投递结果；结果可以是 Delivered、Failed 或 Abandoned。
    pub fn settle(&self, record: &RemoteNotificationOutboxRecord) -> AxonResult<()> {
        if !matches!(
            record.status,
            RemoteNotificationDispatchStatus::Delivered
                | RemoteNotificationDispatchStatus::Failed
                | RemoteNotificationDispatchStatus::Abandoned
        ) {
            return Err(AxonError::runtime_failure("DeliveryStore 结算状态无效"));
        }
        self.persist_outcome(record)
    }

    /// 写入可重试失败结果；重试时间和次数由 outbox 校验统一约束。
    pub fn retry(&self, record: &RemoteNotificationOutboxRecord) -> AxonResult<()> {
        if record.status != RemoteNotificationDispatchStatus::Failed
            || record.attempt_count >= REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS
            || record.next_retry_after_ms.is_none()
        {
            return Err(AxonError::runtime_failure("DeliveryStore retry 状态无效"));
        }
        self.persist_outcome(record)
    }

    /// 落盘投递结果并保证 revision 严格递增。
    ///
    /// outbox 只接受 revision 不小于当前值的写入，而领取检查点会把 revision 推进到
    /// 不早于发送时刻。若结果与领取同毫秒，结果写入会被静默丢弃，记录将停留在
    /// Attempting 并在 stale 恢复后被再次投递。这里显式推进 revision，保证结果必然落盘。
    fn persist_outcome(&self, record: &RemoteNotificationOutboxRecord) -> AxonResult<()> {
        let mut outcome = record.clone();
        if let Some(current) = crate::RemoteStateStoreRuntime::read::<RemoteNotificationOutboxRecord>(
            &self.path,
            &record.dispatch_id,
        )? {
            outcome.updated_at_ms = outcome
                .updated_at_ms
                .max(current.updated_at_ms.saturating_add(1));
        }
        self.append(&outcome).map(|_| ())
    }

    /// 恢复超时的 Attempting 记录，允许渠道 worker 在崩溃后继续领取。
    pub fn recover_stale_attempts(&self, now_ms: u64, stale_after_ms: u64) -> AxonResult<usize> {
        RemoteNotificationOutboxRuntime::recover_stale_attempts(&self.path, now_ms, stale_after_ms)
    }
}

fn is_pending(record: &RemoteNotificationOutboxRecord, now_ms: u64) -> bool {
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
