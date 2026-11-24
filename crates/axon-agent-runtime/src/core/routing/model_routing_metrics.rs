//! 模型路由度量驱动升级（AXON-P2-3）。
//!
//! 将关键词路由升级为度量驱动：采集实测 latency/success/cost，
//! 用 EWMA 平滑后反馈到路由打分，超越 codex/opencode 的静态路由。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// EWMA 平滑系数（alpha）：新样本权重。
/// 0.3 表示新样本占 30%，历史占 70%，平衡响应速度与稳定性。
const EWMA_ALPHA: f64 = 0.3;

/// 最小样本数阈值：低于此值时不参与度量驱动路由。
const MIN_SAMPLE_COUNT: u32 = 3;

/// 成功率基准（百分比）：低于此值的渠道会被惩罚。
const SUCCESS_RATE_BASELINE: f64 = 95.0;

/// 延迟权重系数：延迟对评分的影响权重。
const LATENCY_WEIGHT: f64 = 0.4;

/// 成功率权重系数：成功率对评分的影响权重。
const SUCCESS_WEIGHT: f64 = 0.4;

/// 成本权重系数：成本对评分的影响权重。
const COST_WEIGHT: f64 = 0.2;
/// 每个渠道保留的延迟样本上限，避免进程级指标无限增长。
const MAX_LATENCY_SAMPLES: usize = 64;
/// 统一渠道度量 key 的分隔符。
const CHANNEL_METRIC_KEY_SEPARATOR: &str = "|";

/// 渠道实测度量。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelMetrics {
    /// 渠道标识（model_id + provider）。
    pub channel_key: String,
    /// 平均延迟（毫秒），EWMA 平滑。
    pub avg_latency_ms: f64,
    /// p95 延迟（毫秒），基于最近有限窗口。
    pub p95_latency_ms: f64,
    /// 成功率（百分比 0-100），EWMA 平滑。
    pub success_rate_pct: f64,
    /// 缓存读取命中率（basis points，10000 表示 100%）。
    pub cache_hit_rate_basis_points: u16,
    /// 平均成本（每 1K token），EWMA 平滑。
    pub avg_cost_per_1k_tokens: f64,
    /// 样本数。
    pub sample_count: u32,
    /// 最后更新时间戳（毫秒）。
    pub last_updated_ms: u64,
}

impl Default for ChannelMetrics {
    fn default() -> Self {
        Self {
            channel_key: String::new(),
            avg_latency_ms: 0.0,
            p95_latency_ms: 0.0,
            success_rate_pct: 100.0,
            cache_hit_rate_basis_points: 0,
            avg_cost_per_1k_tokens: 0.0,
            sample_count: 0,
            last_updated_ms: 0,
        }
    }
}

/// 单次请求样本。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequestSample {
    /// 渠道标识。
    pub channel_key: String,
    /// 延迟（毫秒）。
    pub latency_ms: u64,
    /// 是否成功。
    pub success: bool,
    /// 输入 token 数。
    pub input_tokens: u64,
    /// 输出 token 数。
    pub output_tokens: u64,
    /// 总成本（实际货币单位或虚拟成本）。
    pub total_cost: f64,
    /// 时间戳（毫秒）。
    pub timestamp_ms: u64,
}

/// 度量驱动路由评分结果。
#[derive(Debug, Clone, PartialEq)]
pub struct MetricsDrivenScore {
    /// 基础评分（来自关键词路由）。
    pub base_score: u32,
    /// 度量调整后的评分（越小越优先）。
    pub adjusted_score: f64,
    /// 延迟分量。
    pub latency_component: f64,
    /// 成功率分量。
    pub success_component: f64,
    /// 成本分量。
    pub cost_component: f64,
    /// 是否使用了度量数据（样本数不足时退化为基础评分）。
    pub metrics_applied: bool,
}

/// 度量驱动路由 runtime。
#[derive(Debug, Clone)]
pub struct MetricsRouter {
    /// 渠道度量存储。
    metrics: HashMap<String, ChannelMetrics>,
    /// 渠道最近延迟样本，用于计算 p95。
    latency_samples: HashMap<String, VecDeque<u64>>,
}

impl Default for MetricsRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsRouter {
    /// 创建新的度量路由器。
    #[must_use]
    pub fn new() -> Self {
        Self {
            metrics: HashMap::new(),
            latency_samples: HashMap::new(),
        }
    }

    /// 记录单次请求样本，更新 EWMA。
    pub fn record_sample(&mut self, sample: RequestSample) {
        let channel_metrics = self
            .metrics
            .entry(sample.channel_key.clone())
            .or_insert_with(|| ChannelMetrics {
                channel_key: sample.channel_key.clone(),
                ..Default::default()
            });

        // EWMA 更新延迟
        if channel_metrics.sample_count == 0 {
            channel_metrics.avg_latency_ms = sample.latency_ms as f64;
        } else {
            channel_metrics.avg_latency_ms = EWMA_ALPHA * (sample.latency_ms as f64)
                + (1.0 - EWMA_ALPHA) * channel_metrics.avg_latency_ms;
        }

        // EWMA 更新成功率
        let success_value = if sample.success { 100.0 } else { 0.0 };
        if channel_metrics.sample_count == 0 {
            channel_metrics.success_rate_pct = success_value;
        } else {
            channel_metrics.success_rate_pct =
                EWMA_ALPHA * success_value + (1.0 - EWMA_ALPHA) * channel_metrics.success_rate_pct;
        }

        // EWMA 更新成本
        let total_tokens = sample.input_tokens + sample.output_tokens;
        if total_tokens > 0 {
            let cost_per_1k = (sample.total_cost / total_tokens as f64) * 1000.0;
            if channel_metrics.sample_count == 0 {
                channel_metrics.avg_cost_per_1k_tokens = cost_per_1k;
            } else {
                channel_metrics.avg_cost_per_1k_tokens = EWMA_ALPHA * cost_per_1k
                    + (1.0 - EWMA_ALPHA) * channel_metrics.avg_cost_per_1k_tokens;
            }
        }

        channel_metrics.sample_count = channel_metrics.sample_count.saturating_add(1);
        channel_metrics.last_updated_ms = sample.timestamp_ms;
        let samples = self.latency_samples.entry(sample.channel_key).or_default();
        samples.push_back(sample.latency_ms);
        while samples.len() > MAX_LATENCY_SAMPLES {
            samples.pop_front();
        }
        channel_metrics.p95_latency_ms = percentile95(samples);
    }

    /// 记录一次缓存命中率观测，不增加请求样本计数。
    pub fn record_cache_hit_rate(
        &mut self,
        channel_key: &str,
        cache_hit_rate_basis_points: u16,
        timestamp_ms: u64,
    ) {
        let metrics = self
            .metrics
            .entry(channel_key.to_owned())
            .or_insert_with(|| ChannelMetrics {
                channel_key: channel_key.to_owned(),
                ..Default::default()
            });
        if metrics.sample_count == 0 {
            metrics.cache_hit_rate_basis_points = cache_hit_rate_basis_points;
        } else {
            let current = f64::from(metrics.cache_hit_rate_basis_points);
            let observed = f64::from(cache_hit_rate_basis_points);
            metrics.cache_hit_rate_basis_points = (0.3 * observed + 0.7 * current).round() as u16;
        }
        metrics.last_updated_ms = timestamp_ms;
    }

    /// 计算度量驱动的路由评分。
    ///
    /// 如果样本数不足，退化为基础评分；否则综合延迟、成功率、成本计算。
    #[must_use]
    pub fn compute_score(&self, channel_key: &str, base_score: u32) -> MetricsDrivenScore {
        let Some(metrics) = self.metrics.get(channel_key) else {
            return MetricsDrivenScore {
                base_score,
                adjusted_score: f64::from(base_score),
                latency_component: 0.0,
                success_component: 0.0,
                cost_component: 0.0,
                metrics_applied: false,
            };
        };

        if metrics.sample_count < MIN_SAMPLE_COUNT {
            return MetricsDrivenScore {
                base_score,
                adjusted_score: f64::from(base_score),
                latency_component: 0.0,
                success_component: 0.0,
                cost_component: 0.0,
                metrics_applied: false,
            };
        }

        // 延迟分量：优先使用最近窗口 p95，1000ms 为基准
        let latency_ms = if metrics.p95_latency_ms > 0.0 {
            metrics.p95_latency_ms
        } else {
            metrics.avg_latency_ms
        };
        let latency_component = (latency_ms / 1000.0).min(10.0);

        // 成功率分量：低于基准时惩罚，高于基准时奖励
        let success_component = if metrics.success_rate_pct < SUCCESS_RATE_BASELINE {
            (SUCCESS_RATE_BASELINE - metrics.success_rate_pct) / 10.0
        } else {
            0.0
        };

        // 成本分量：归一化到 0-1，0.01 为基准（每 1K token）
        let cost_component = (metrics.avg_cost_per_1k_tokens / 0.01).min(10.0);

        // 综合评分：base_score + 加权度量
        let adjusted_score = f64::from(base_score)
            + LATENCY_WEIGHT * latency_component
            + SUCCESS_WEIGHT * success_component
            + COST_WEIGHT * cost_component;

        MetricsDrivenScore {
            base_score,
            adjusted_score,
            latency_component,
            success_component,
            cost_component,
            metrics_applied: true,
        }
    }

    /// 获取渠道度量（只读）。
    #[must_use]
    pub fn get_metrics(&self, channel_key: &str) -> Option<&ChannelMetrics> {
        self.metrics.get(channel_key)
    }

    /// 列出所有渠道度量。
    #[must_use]
    pub fn all_metrics(&self) -> Vec<&ChannelMetrics> {
        self.metrics.values().collect()
    }

    /// 获取所有渠道度量快照。
    #[must_use]
    pub fn snapshot(&self) -> Vec<ChannelMetrics> {
        self.metrics.values().cloned().collect()
    }

    /// 清除过期度量（超过指定时间未更新）。
    pub fn prune_stale(&mut self, now_ms: u64, max_age_ms: u64) {
        self.metrics
            .retain(|_, metrics| now_ms.saturating_sub(metrics.last_updated_ms) < max_age_ms);
        self.latency_samples
            .retain(|channel_key, _| self.metrics.contains_key(channel_key));
    }
}

/// 构建跨路由选择与度量记录复用的稳定渠道 key。
#[must_use]
pub fn channel_metrics_key(routing_key: &str, provider: &str) -> String {
    format!("{routing_key}{CHANNEL_METRIC_KEY_SEPARATOR}{provider}")
}

fn percentile95(samples: &VecDeque<u64>) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.iter().copied().collect::<Vec<_>>();
    sorted.sort_unstable();
    let index = ((sorted.len().saturating_sub(1) * 95) / 100).min(sorted.len() - 1);
    sorted[index] as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_router_ewma_smoothing() {
        let mut router = MetricsRouter::new();

        // 第一个样本
        router.record_sample(RequestSample {
            channel_key: "test-channel".to_owned(),
            latency_ms: 1000,
            success: true,
            input_tokens: 1000,
            output_tokens: 1000,
            total_cost: 0.02,
            timestamp_ms: 1000,
        });

        // 第二个样本（更快）
        router.record_sample(RequestSample {
            channel_key: "test-channel".to_owned(),
            latency_ms: 400,
            success: true,
            input_tokens: 1000,
            output_tokens: 1000,
            total_cost: 0.02,
            timestamp_ms: 2000,
        });

        let metrics = router.get_metrics("test-channel").unwrap();
        assert_eq!(metrics.sample_count, 2);
        // EWMA: 0.3 * 400 + 0.7 * 1000 = 820
        assert!((metrics.avg_latency_ms - 820.0).abs() < 0.1);
    }

    #[test]
    fn metrics_router_compute_score_with_samples() {
        let mut router = MetricsRouter::new();

        // 记录 3 个样本（达到最小阈值）
        for i in 0..3 {
            router.record_sample(RequestSample {
                channel_key: "test-channel".to_owned(),
                latency_ms: 500,
                success: true,
                input_tokens: 1000,
                output_tokens: 1000,
                total_cost: 0.02,
                timestamp_ms: 1000 + i * 100,
            });
        }

        let score = router.compute_score("test-channel", 10);

        assert_eq!(score.base_score, 10);
        assert!(score.metrics_applied);
        assert!(score.adjusted_score > 10.0); // 应该有度量调整
    }

    #[test]
    fn metrics_router_penalizes_low_success_rate() {
        let mut router = MetricsRouter::new();

        // 记录失败样本
        for i in 0..5 {
            router.record_sample(RequestSample {
                channel_key: "unreliable-channel".to_owned(),
                latency_ms: 500,
                success: i % 2 == 0, // 60% 成功率
                input_tokens: 1000,
                output_tokens: 1000,
                total_cost: 0.02,
                timestamp_ms: 1000 + i * 100,
            });
        }

        let score = router.compute_score("unreliable-channel", 10);

        assert!(score.metrics_applied);
        assert!(score.success_component > 0.0); // 应该有成功率惩罚
        assert!(score.adjusted_score > f64::from(score.base_score));
    }

    #[test]
    fn metrics_router_prunes_stale() {
        let mut router = MetricsRouter::new();

        router.record_sample(RequestSample {
            channel_key: "old-channel".to_owned(),
            latency_ms: 500,
            success: true,
            input_tokens: 1000,
            output_tokens: 1000,
            total_cost: 0.02,
            timestamp_ms: 1000,
        });

        router.record_sample(RequestSample {
            channel_key: "new-channel".to_owned(),
            latency_ms: 500,
            success: true,
            input_tokens: 1000,
            output_tokens: 1000,
            total_cost: 0.02,
            timestamp_ms: 10000,
        });

        // 清除超过 5000ms 未更新的
        router.prune_stale(12000, 5000);

        assert!(router.get_metrics("old-channel").is_none());
        assert!(router.get_metrics("new-channel").is_some());
    }
}
