use std::collections::HashMap;

use axon_domain::BreakerPolicy;

/// 渠道标识：以 `(model_name, provider)` 唯一定位一个上游渠道的健康状态。
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct ChannelKey {
    /// 负载均衡路由键（model.name）。
    pub model_name: String,
    /// 渠道 provider 名称。
    pub provider: String,
}

/// 渠道健康注册表：按 `ChannelKey` 管理每个渠道的失败计数与冷却窗口。
///
/// 单线程进程模型，`&mut self` 接口无锁。`HashMap` 仅在渠道首次失败时
/// 按需创建条目，健康渠道零额外分配。
#[derive(Debug, Clone)]
pub struct ChannelHealthRegistry {
    failures: HashMap<ChannelKey, ChannelFailureState>,
    policy: BreakerPolicy,
}

#[derive(Debug, Clone, Default)]
struct ChannelFailureState {
    consecutive_failures: u32,
    blocked_until_ms: Option<u64>,
}

impl ChannelHealthRegistry {
    /// 以给定熔断策略创建空注册表。
    #[must_use]
    pub fn new(policy: BreakerPolicy) -> Self {
        Self {
            failures: HashMap::new(),
            policy,
        }
    }

    /// 渠道当前是否可用（放行）。
    #[must_use]
    pub fn is_available(&mut self, key: &ChannelKey, now_ms: u64) -> bool {
        match self
            .failures
            .get(key)
            .and_then(|state| state.blocked_until_ms)
        {
            Some(blocked_until_ms) => now_ms >= blocked_until_ms,
            None => true,
        }
    }

    /// 记录一次渠道成功：清除失败状态（不存在则跳过，保持零开销）。
    pub fn record_success(&mut self, key: &ChannelKey) {
        self.failures.remove(key);
    }

    /// 记录一次渠道失败：按需创建状态并累加失败计数，达到阈值后进入冷却窗口。
    pub fn record_failure(&mut self, key: &ChannelKey, now_ms: u64) {
        let state = self.failures.entry(key.clone()).or_default();
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);

        let threshold = self.policy.failure_threshold.max(1);
        if state.consecutive_failures >= threshold {
            state.blocked_until_ms = Some(now_ms.saturating_add(self.policy.cooldown_ms));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_channel_health_registry() {
        let policy = BreakerPolicy {
            failure_threshold: 2,
            cooldown_ms: 5000,
        };
        let mut registry = ChannelHealthRegistry::new(policy);
        let key_a = ChannelKey {
            model_name: "gpt-4o".to_string(),
            provider: "openai".to_string(),
        };
        let key_b = ChannelKey {
            model_name: "claude-3-5".to_string(),
            provider: "anthropic".to_string(),
        };

        // 默认都可用
        assert!(registry.is_available(&key_a, 1000));
        assert!(registry.is_available(&key_b, 1000));

        // key_a 连续失败 2 次导致进入冷却窗口
        registry.record_failure(&key_a, 1000);
        assert!(registry.is_available(&key_a, 1050));
        registry.record_failure(&key_a, 1100);

        // A 冷却中，B 依然可用
        assert!(!registry.is_available(&key_a, 1200));
        assert!(registry.is_available(&key_b, 1200));

        // A 冷却时间过去后恢复放行
        assert!(registry.is_available(&key_a, 6200));

        // 冷却后再次失败会立即进入新的冷却窗口
        registry.record_failure(&key_a, 6300);
        assert!(!registry.is_available(&key_a, 6301));

        // 成功后清除失败状态
        registry.record_success(&key_a);
        assert!(registry.is_available(&key_a, 6400));
    }

    #[test]
    fn zero_failure_threshold_behaves_as_one() {
        let policy = BreakerPolicy {
            failure_threshold: 0,
            cooldown_ms: 5000,
        };
        let mut registry = ChannelHealthRegistry::new(policy);
        let key = ChannelKey {
            model_name: "gpt-4o".to_string(),
            provider: "openai".to_string(),
        };

        registry.record_failure(&key, 1000);
        assert!(!registry.is_available(&key, 1001));
    }
}
