//! TUI 有界邮箱的 trace-only 聚合遥测。

use std::{
    array,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};

const QUEUE_WAIT_BUCKET_COUNT: usize = 32;
const PERCENTILE_SCALE: u64 = 100;
const P50: u64 = 50;
const P95: u64 = 95;
const P99: u64 = 99;

/// 单条邮箱链路的聚合快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxLaneTelemetrySnapshot {
    /// 实际进入队列的元素数。
    pub enqueued_count: u64,
    /// 从队列取出的元素数。
    pub drained_count: u64,
    /// 被合并进已有元素的输入数。
    pub coalesced_count: u64,
    /// 因序号迟到（不高于已投影水位）被拒绝的 wire 帧数。
    #[serde(default)]
    pub stale_count: u64,
    /// 因背压淘汰的可重建展示增量帧数。
    pub shed_count: u64,
    /// 观测到的最大待处理元素数。
    pub backlog_high_water: u64,
    /// 排队耗时 P50（微秒，固定桶上界）。
    pub queue_wait_p50_us: Option<u64>,
    /// 排队耗时 P95（微秒，固定桶上界）。
    pub queue_wait_p95_us: Option<u64>,
    /// 排队耗时 P99（微秒，固定桶上界）。
    pub queue_wait_p99_us: Option<u64>,
}

/// TUI worker 与 runtime mailbox 的聚合快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxTelemetrySnapshot {
    /// 是否启用了采集。
    pub enabled: bool,
    /// submission worker mailbox 指标。
    pub worker: MailboxLaneTelemetrySnapshot,
    /// runtime event mailbox 指标。
    pub runtime: MailboxLaneTelemetrySnapshot,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MailboxLane {
    Worker,
}

#[derive(Debug, Clone)]
pub(crate) struct MailboxTelemetry {
    enabled: bool,
    inner: Arc<MailboxTelemetryInner>,
}

#[derive(Debug)]
struct MailboxTelemetryInner {
    worker: MailboxLaneTelemetry,
    runtime: MailboxLaneTelemetry,
}

#[derive(Debug)]
struct MailboxLaneTelemetry {
    enqueued_count: AtomicU64,
    drained_count: AtomicU64,
    coalesced_count: AtomicU64,
    stale_count: AtomicU64,
    shed_count: AtomicU64,
    backlog_high_water: AtomicU64,
    queue_wait_buckets: [AtomicU64; QUEUE_WAIT_BUCKET_COUNT],
}

impl MailboxTelemetry {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            inner: Arc::new(MailboxTelemetryInner {
                worker: MailboxLaneTelemetry::new(),
                runtime: MailboxLaneTelemetry::new(),
            }),
        }
    }

    pub(crate) fn from_config() -> Self {
        Self::new(axon_session_runtime::first_token_telemetry_enabled())
    }

    pub(crate) fn enqueued_at(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(crate) fn record_enqueued(&self, lane: MailboxLane, backlog: usize) {
        let Some(metrics) = self.metrics(lane) else {
            return;
        };
        metrics.enqueued_count.fetch_add(1, Ordering::Relaxed);
        update_max(
            &metrics.backlog_high_water,
            u64::try_from(backlog).unwrap_or(u64::MAX),
        );
    }

    pub(crate) fn record_drained(&self, lane: MailboxLane, enqueued_at: Option<Instant>) {
        let Some(metrics) = self.metrics(lane) else {
            return;
        };
        metrics.drained_count.fetch_add(1, Ordering::Relaxed);
        if let Some(enqueued_at) = enqueued_at {
            let wait_us = u64::try_from(enqueued_at.elapsed().as_micros()).unwrap_or(u64::MAX);
            metrics.queue_wait_buckets[queue_wait_bucket(wait_us)].fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_coalesced(&self, lane: MailboxLane) {
        if let Some(metrics) = self.metrics(lane) {
            metrics.coalesced_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_stale(&self, lane: MailboxLane) {
        if let Some(metrics) = self.metrics(lane) {
            metrics.stale_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_shed(&self, lane: MailboxLane) {
        if let Some(metrics) = self.metrics(lane) {
            metrics.shed_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn snapshot(&self) -> MailboxTelemetrySnapshot {
        if !self.enabled {
            return MailboxTelemetrySnapshot::default();
        }
        MailboxTelemetrySnapshot {
            enabled: true,
            worker: self.inner.worker.snapshot(),
            runtime: self.inner.runtime.snapshot(),
        }
    }

    fn metrics(&self, lane: MailboxLane) -> Option<&MailboxLaneTelemetry> {
        let MailboxLane::Worker = lane;
        self.enabled.then_some(&self.inner.worker)
    }
}

impl Default for MailboxTelemetry {
    fn default() -> Self {
        Self::new(false)
    }
}

impl MailboxLaneTelemetry {
    fn new() -> Self {
        Self {
            enqueued_count: AtomicU64::new(0),
            drained_count: AtomicU64::new(0),
            coalesced_count: AtomicU64::new(0),
            stale_count: AtomicU64::new(0),
            shed_count: AtomicU64::new(0),
            backlog_high_water: AtomicU64::new(0),
            queue_wait_buckets: array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn snapshot(&self) -> MailboxLaneTelemetrySnapshot {
        let buckets = self
            .queue_wait_buckets
            .each_ref()
            .map(|bucket| bucket.load(Ordering::Relaxed));
        MailboxLaneTelemetrySnapshot {
            enqueued_count: self.enqueued_count.load(Ordering::Relaxed),
            drained_count: self.drained_count.load(Ordering::Relaxed),
            coalesced_count: self.coalesced_count.load(Ordering::Relaxed),
            stale_count: self.stale_count.load(Ordering::Relaxed),
            shed_count: self.shed_count.load(Ordering::Relaxed),
            backlog_high_water: self.backlog_high_water.load(Ordering::Relaxed),
            queue_wait_p50_us: histogram_percentile(&buckets, P50),
            queue_wait_p95_us: histogram_percentile(&buckets, P95),
            queue_wait_p99_us: histogram_percentile(&buckets, P99),
        }
    }
}

fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(observed) => current = observed,
        }
    }
}

fn queue_wait_bucket(wait_us: u64) -> usize {
    if wait_us <= 1 {
        return 0;
    }
    usize::try_from(u64::BITS - (wait_us - 1).leading_zeros())
        .unwrap_or(QUEUE_WAIT_BUCKET_COUNT - 1)
        .min(QUEUE_WAIT_BUCKET_COUNT - 1)
}

fn histogram_percentile(buckets: &[u64; QUEUE_WAIT_BUCKET_COUNT], percentile: u64) -> Option<u64> {
    let total = buckets.iter().copied().sum::<u64>();
    if total == 0 {
        return None;
    }
    let rank = total
        .saturating_mul(percentile)
        .saturating_add(PERCENTILE_SCALE - 1)
        / PERCENTILE_SCALE;
    let mut cumulative = 0u64;
    let index = buckets
        .iter()
        .position(|count| {
            cumulative = cumulative.saturating_add(*count);
            cumulative >= rank
        })
        .unwrap_or(QUEUE_WAIT_BUCKET_COUNT - 1);
    Some(bucket_upper_bound_us(index))
}

fn bucket_upper_bound_us(index: usize) -> u64 {
    if index == 0 {
        1
    } else {
        1u64.checked_shl(u32::try_from(index).unwrap_or(u32::MAX))
            .unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_collector_remains_empty() {
        let telemetry = MailboxTelemetry::new(false);
        telemetry.record_enqueued(MailboxLane::Worker, 3);
        telemetry.record_coalesced(MailboxLane::Worker);

        assert_eq!(telemetry.enqueued_at(), None);
        assert_eq!(telemetry.snapshot(), MailboxTelemetrySnapshot::default());
    }

    #[test]
    fn enabled_collector_tracks_bounded_distribution() {
        let telemetry = MailboxTelemetry::new(true);
        telemetry.record_enqueued(MailboxLane::Worker, 4);
        telemetry.record_drained(MailboxLane::Worker, telemetry.enqueued_at());
        telemetry.record_coalesced(MailboxLane::Worker);
        telemetry.record_stale(MailboxLane::Worker);
        telemetry.record_shed(MailboxLane::Worker);

        let snapshot = telemetry.snapshot();
        assert!(snapshot.enabled);
        assert_eq!(snapshot.worker.enqueued_count, 1);
        assert_eq!(snapshot.worker.drained_count, 1);
        assert_eq!(snapshot.worker.coalesced_count, 1);
        assert_eq!(snapshot.worker.stale_count, 1);
        assert_eq!(snapshot.worker.shed_count, 1);
        assert_eq!(snapshot.worker.backlog_high_water, 4);
        assert!(snapshot.worker.queue_wait_p99_us.is_some());
    }
}
