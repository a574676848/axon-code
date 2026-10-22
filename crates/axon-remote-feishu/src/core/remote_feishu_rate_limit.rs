//! Feishu WebSocket 入站限流。

use std::collections::{BTreeMap, VecDeque};

use axon_domain::{AxonError, AxonResult};

/// 默认限流窗口，单位毫秒。
pub const REMOTE_FEISHU_RATE_LIMIT_WINDOW_MS: u64 = 60_000;
/// 默认每窗口最大事件数。
pub const REMOTE_FEISHU_RATE_LIMIT_MAX_EVENTS: usize = 120;
/// 默认最大跟踪 key 数。
pub const REMOTE_FEISHU_RATE_LIMIT_MAX_KEYS: usize = 4096;

/// Feishu 入站限流决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuRateLimitDecision {
    /// 是否允许通过。
    pub allowed: bool,
    /// 当前窗口内事件数。
    pub events_in_window: usize,
    /// 下一次可重试时间。
    pub retry_after_ms: Option<u64>,
}

/// Feishu 入站限流 runtime。
#[derive(Debug, Clone)]
pub struct RemoteFeishuRateLimiter {
    window_ms: u64,
    max_events: usize,
    max_keys: usize,
    events: BTreeMap<String, VecDeque<u64>>,
    access_order: VecDeque<String>,
}

impl RemoteFeishuRateLimiter {
    /// 创建限流器。
    pub fn new(window_ms: u64, max_events: usize, max_keys: usize) -> AxonResult<Self> {
        if window_ms == 0 || max_events == 0 || max_keys == 0 {
            return Err(AxonError::runtime_failure("Feishu 限流配置无效"));
        }
        Ok(Self {
            window_ms,
            max_events,
            max_keys,
            events: BTreeMap::new(),
            access_order: VecDeque::new(),
        })
    }

    /// 记录一次事件并返回限流决策。
    pub fn check(&mut self, key: &str, now_ms: u64) -> AxonResult<RemoteFeishuRateLimitDecision> {
        if key.trim().is_empty() || now_ms == 0 {
            return Err(AxonError::runtime_failure("Feishu 限流输入无效"));
        }
        self.ensure_key_capacity(key);
        self.touch_key(key);
        let window_ms = self.window_ms;
        let max_events = self.max_events;
        let queue = self.events.entry(key.to_owned()).or_default();
        while queue
            .front()
            .is_some_and(|recorded_at_ms| now_ms.saturating_sub(*recorded_at_ms) >= window_ms)
        {
            queue.pop_front();
        }
        if queue.len() >= max_events {
            let retry_after_ms = queue.front().map(|oldest| oldest.saturating_add(window_ms));
            return Ok(RemoteFeishuRateLimitDecision {
                allowed: false,
                events_in_window: queue.len(),
                retry_after_ms,
            });
        }
        queue.push_back(now_ms);
        Ok(RemoteFeishuRateLimitDecision {
            allowed: true,
            events_in_window: queue.len(),
            retry_after_ms: None,
        })
    }

    fn ensure_key_capacity(&mut self, incoming_key: &str) {
        if self.events.contains_key(incoming_key) || self.events.len() < self.max_keys {
            return;
        }
        while let Some(least_recently_used) = self.access_order.pop_front() {
            if self.events.remove(&least_recently_used).is_some() {
                break;
            }
        }
    }

    fn touch_key(&mut self, key: &str) {
        if let Some(index) = self
            .access_order
            .iter()
            .position(|candidate| candidate == key)
        {
            self.access_order.remove(index);
        }
        self.access_order.push_back(key.to_owned());
    }
}

impl Default for RemoteFeishuRateLimiter {
    fn default() -> Self {
        Self::new(
            REMOTE_FEISHU_RATE_LIMIT_WINDOW_MS,
            REMOTE_FEISHU_RATE_LIMIT_MAX_EVENTS,
            REMOTE_FEISHU_RATE_LIMIT_MAX_KEYS,
        )
        .expect("default Feishu rate limiter config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_blocks_after_window_budget_is_used() {
        let mut limiter = RemoteFeishuRateLimiter::new(1_000, 2, 16).expect("limiter");

        assert!(limiter.check("oc_1", 100).expect("first").allowed);
        assert!(limiter.check("oc_1", 200).expect("second").allowed);
        let blocked = limiter.check("oc_1", 300).expect("third");

        assert!(!blocked.allowed);
        assert_eq!(blocked.retry_after_ms, Some(1_100));
    }

    #[test]
    fn rate_limiter_releases_after_window_slides() {
        let mut limiter = RemoteFeishuRateLimiter::new(1_000, 1, 16).expect("limiter");

        assert!(limiter.check("oc_1", 100).expect("first").allowed);
        assert!(limiter.check("oc_1", 1_100).expect("after window").allowed);
    }

    #[test]
    fn rate_limiter_evicts_least_recently_used_key() {
        let mut limiter = RemoteFeishuRateLimiter::new(1_000, 10, 2).expect("limiter");
        limiter.check("key-a", 100).expect("key a");
        limiter.check("key-b", 200).expect("key b");
        limiter.check("key-a", 300).expect("refresh key a");
        limiter.check("key-c", 400).expect("key c");

        assert!(limiter.events.contains_key("key-a"));
        assert!(!limiter.events.contains_key("key-b"));
        assert!(limiter.events.contains_key("key-c"));
    }
}
