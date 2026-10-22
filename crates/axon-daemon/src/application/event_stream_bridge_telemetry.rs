//! EventStreamBridge 的 trace-only 聚合遥测。

use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};

/// EventStreamBridge 聚合快照。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventStreamBridgeTelemetrySnapshot {
    /// 是否启用了采集。
    pub enabled: bool,
    /// 当前订阅者数。
    pub subscriber_count: u64,
    /// 订阅者数量峰值。
    pub subscriber_high_water: u64,
    /// 事件发布调用数。
    pub publish_count: u64,
    /// Interaction 匹配后的 fan-out 总数。
    pub matched_fan_out_count: u64,
    /// 成功投递数。
    pub delivered_count: u64,
    /// mailbox full 导致的订阅淘汰数。
    pub mailbox_full_prune_count: u64,
    /// 接收端断开导致的订阅淘汰数。
    pub disconnected_prune_count: u64,
    /// generation 切换导致的订阅淘汰数。
    pub generation_prune_count: u64,
    /// publish 路径获取 registry lock 的次数。
    pub lock_wait_count: u64,
    /// registry lock 累计等待时间（微秒）。
    pub lock_wait_total_us: u64,
    /// registry lock 最大等待时间（微秒）。
    pub lock_wait_max_us: u64,
}

#[derive(Debug, Clone)]
pub(super) struct EventStreamBridgeTelemetry {
    enabled: bool,
    inner: Arc<EventStreamBridgeTelemetryInner>,
}

#[derive(Debug, Default)]
struct EventStreamBridgeTelemetryInner {
    subscriber_count: AtomicU64,
    subscriber_high_water: AtomicU64,
    publish_count: AtomicU64,
    matched_fan_out_count: AtomicU64,
    delivered_count: AtomicU64,
    mailbox_full_prune_count: AtomicU64,
    disconnected_prune_count: AtomicU64,
    generation_prune_count: AtomicU64,
    lock_wait_count: AtomicU64,
    lock_wait_total_us: AtomicU64,
    lock_wait_max_us: AtomicU64,
}

impl EventStreamBridgeTelemetry {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            inner: Arc::new(EventStreamBridgeTelemetryInner::default()),
        }
    }

    pub(super) fn lock_started_at(&self) -> Option<Instant> {
        if self.enabled {
            Some(Instant::now())
        } else {
            None
        }
    }

    pub(super) fn record_lock_acquired(&self, started_at: Option<Instant>) {
        let Some(started_at) = started_at else {
            return;
        };
        let wait_us = u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
        self.inner.lock_wait_count.fetch_add(1, Ordering::Relaxed);
        self.inner
            .lock_wait_total_us
            .fetch_add(wait_us, Ordering::Relaxed);
        update_max(&self.inner.lock_wait_max_us, wait_us);
    }

    pub(super) fn record_subscriber_added(&self) {
        if !self.enabled {
            return;
        }
        let count = self
            .inner
            .subscriber_count
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        update_max(&self.inner.subscriber_high_water, count);
    }

    pub(super) fn record_subscribers_removed(&self, count: usize) {
        if !self.enabled || count == 0 {
            return;
        }
        let count = u64::try_from(count).unwrap_or(u64::MAX);
        let _ = self.inner.subscriber_count.fetch_update(
            Ordering::Relaxed,
            Ordering::Relaxed,
            |current| Some(current.saturating_sub(count)),
        );
    }

    pub(super) fn record_publish(&self) {
        if self.enabled {
            self.inner.publish_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_match(&self) {
        if self.enabled {
            self.inner
                .matched_fan_out_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_delivered(&self) {
        if self.enabled {
            self.inner.delivered_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_mailbox_full_prune(&self) {
        if self.enabled {
            self.inner
                .mailbox_full_prune_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_disconnected_prune(&self) {
        if self.enabled {
            self.inner
                .disconnected_prune_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn record_generation_prune(&self) {
        if self.enabled {
            self.inner
                .generation_prune_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(super) fn snapshot(&self) -> EventStreamBridgeTelemetrySnapshot {
        if !self.enabled {
            return EventStreamBridgeTelemetrySnapshot::default();
        }
        EventStreamBridgeTelemetrySnapshot {
            enabled: true,
            subscriber_count: self.inner.subscriber_count.load(Ordering::Relaxed),
            subscriber_high_water: self.inner.subscriber_high_water.load(Ordering::Relaxed),
            publish_count: self.inner.publish_count.load(Ordering::Relaxed),
            matched_fan_out_count: self.inner.matched_fan_out_count.load(Ordering::Relaxed),
            delivered_count: self.inner.delivered_count.load(Ordering::Relaxed),
            mailbox_full_prune_count: self.inner.mailbox_full_prune_count.load(Ordering::Relaxed),
            disconnected_prune_count: self.inner.disconnected_prune_count.load(Ordering::Relaxed),
            generation_prune_count: self.inner.generation_prune_count.load(Ordering::Relaxed),
            lock_wait_count: self.inner.lock_wait_count.load(Ordering::Relaxed),
            lock_wait_total_us: self.inner.lock_wait_total_us.load(Ordering::Relaxed),
            lock_wait_max_us: self.inner.lock_wait_max_us.load(Ordering::Relaxed),
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
