//! 可观测性领域模型与实时度量聚合算法。
//!
//! 包含：
//! 1. 内存无锁滑动窗口（SlidingWindowRingBuffer：360 槽 × 10 秒 = 60 分钟）；
//! 2. GitHub 风格活动矩阵热力图 5 档分桶算法（纯函数）；
//! 3. 请求健康度 3 档分类算法（纯函数）；
//! 4. 支撑 SSOT §5.2 的只读快照聚合模型。

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/// 内存滑动窗口总槽位数（360 槽 × 10 秒 = 60 分钟）。
pub const SLIDING_WINDOW_SLOT_COUNT: usize = 360;
/// 内存滑动窗口单槽时间跨度（10 秒）。
pub const SLIDING_WINDOW_SLOT_DURATION_MS: u64 = 10_000;
/// 内存滑动窗口总时间跨度（3600 秒 = 60 分钟）。
pub const SLIDING_WINDOW_TOTAL_DURATION_MS: u64 = 3_600_000;

/// 槽位重置中标志位（CAS 最高位守卫）。
const RESETTING_EPOCH_BIT: u64 = 1 << 63;

/// 单次指标增量。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricDelta {
    /// 请求数增量。
    pub request_count: u64,
    /// 成功请求数增量。
    pub success_count: u64,
    /// 失败请求数增量。
    pub failure_count: u64,
    /// 输入 token 总数增量。
    pub total_input_tokens: u64,
    /// 缓存命中读取 token 增量。
    pub cache_read_input_tokens: u64,
    /// 缓存写入 token 增量。
    pub cache_write_input_tokens: u64,
    /// 输出 token 增量。
    pub output_tokens: u64,
    /// 思考 token 增量。
    pub reasoning_output_tokens: u64,
    /// 提供方延迟耗时（毫秒）。
    pub provider_duration_ms: u64,
    /// 提供方调用次数。
    pub provider_calls: u64,
    /// 首字延迟耗时（毫秒）。
    pub ttft_duration_ms: u64,
    /// 测得首字延迟的调用次数。
    pub measured_ttft_count: u64,
    /// 计费成本（微美元）。
    pub cost_micros: u64,
}

/// 单个槽位只读快照。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SlotSnapshot {
    /// 槽位所属 slot_id（以 10 秒为基底）。
    pub slot_id: u64,
    /// 请求数。
    pub request_count: u64,
    /// 成功请求数。
    pub success_count: u64,
    /// 失败请求数。
    pub failure_count: u64,
    /// 输入 token 总数。
    pub total_input_tokens: u64,
    /// 缓存读取 token 数。
    pub cache_read_input_tokens: u64,
    /// 缓存写入 token 数。
    pub cache_write_input_tokens: u64,
    /// 输出 token 数。
    pub output_tokens: u64,
    /// 思考 token 数。
    pub reasoning_output_tokens: u64,
    /// 提供方延迟毫秒。
    pub provider_duration_ms: u64,
    /// 提供方调用次数。
    pub provider_calls: u64,
    /// 首字延迟毫秒。
    pub ttft_duration_ms: u64,
    /// 首字延迟有效样本数。
    pub measured_ttft_count: u64,
    /// 计费成本（微美元）。
    pub cost_micros: u64,
}

/// 内存无锁槽位。
pub struct WindowSlot {
    epoch: AtomicU64,
    request_count: AtomicU64,
    success_count: AtomicU64,
    failure_count: AtomicU64,
    total_input_tokens: AtomicU64,
    cache_read_input_tokens: AtomicU64,
    cache_write_input_tokens: AtomicU64,
    output_tokens: AtomicU64,
    reasoning_output_tokens: AtomicU64,
    provider_duration_ms: AtomicU64,
    provider_calls: AtomicU64,
    ttft_duration_ms: AtomicU64,
    measured_ttft_count: AtomicU64,
    cost_micros: AtomicU64,
}

impl Default for WindowSlot {
    fn default() -> Self {
        Self {
            epoch: AtomicU64::new(0),
            request_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            total_input_tokens: AtomicU64::new(0),
            cache_read_input_tokens: AtomicU64::new(0),
            cache_write_input_tokens: AtomicU64::new(0),
            output_tokens: AtomicU64::new(0),
            reasoning_output_tokens: AtomicU64::new(0),
            provider_duration_ms: AtomicU64::new(0),
            provider_calls: AtomicU64::new(0),
            ttft_duration_ms: AtomicU64::new(0),
            measured_ttft_count: AtomicU64::new(0),
            cost_micros: AtomicU64::new(0),
        }
    }
}

impl WindowSlot {
    /// 原子记录增量指标。
    ///
    /// 通过 epoch 版本 CAS 实现无锁跨轮清零与并发累加，彻底杜绝 Mutex/RwLock。
    pub fn record(&self, target_slot_id: u64, delta: &MetricDelta) {
        loop {
            let current_epoch = self.epoch.load(Ordering::Acquire);

            // 1. 若其他线程正在执行跨轮重置，短暂让步等待
            if (current_epoch & RESETTING_EPOCH_BIT) != 0 {
                std::hint::spin_loop();
                continue;
            }

            // 2. 当前槽位已属于目标 slot_id，直接无锁原子累加
            if current_epoch == target_slot_id {
                self.add_delta(delta);
                if self.epoch.load(Ordering::Acquire) == target_slot_id {
                    return;
                }
                return;
            }

            // 3. 当前槽位属于上一轮或更早历史（或初始 0），需跨轮清零复用
            if current_epoch < target_slot_id {
                let resetting_val = target_slot_id | RESETTING_EPOCH_BIT;
                if self
                    .epoch
                    .compare_exchange_weak(
                        current_epoch,
                        resetting_val,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    // 成功获取清零权，重置槽内所有计数器并赋予初值
                    self.reset_and_set(delta);
                    // 发布正式 epoch，解除重置状态
                    self.epoch.store(target_slot_id, Ordering::Release);
                    return;
                }
                // CAS 争用失败，自旋重试
                continue;
            }

            // 4. 当前槽位 epoch > target_slot_id：传入事实属于已淘汰的历史数据，跳过
            return;
        }
    }

    /// 读取指定 slot_id 的快照。若槽位不匹配或正在重置，返回 None（视为零值）。
    #[must_use]
    pub fn snapshot_at(&self, expected_slot_id: u64) -> Option<SlotSnapshot> {
        let epoch1 = self.epoch.load(Ordering::Acquire);
        if epoch1 != expected_slot_id || (epoch1 & RESETTING_EPOCH_BIT) != 0 {
            return None;
        }

        let snap = SlotSnapshot {
            slot_id: expected_slot_id,
            request_count: self.request_count.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            failure_count: self.failure_count.load(Ordering::Relaxed),
            total_input_tokens: self.total_input_tokens.load(Ordering::Relaxed),
            cache_read_input_tokens: self.cache_read_input_tokens.load(Ordering::Relaxed),
            cache_write_input_tokens: self.cache_write_input_tokens.load(Ordering::Relaxed),
            output_tokens: self.output_tokens.load(Ordering::Relaxed),
            reasoning_output_tokens: self.reasoning_output_tokens.load(Ordering::Relaxed),
            provider_duration_ms: self.provider_duration_ms.load(Ordering::Relaxed),
            provider_calls: self.provider_calls.load(Ordering::Relaxed),
            ttft_duration_ms: self.ttft_duration_ms.load(Ordering::Relaxed),
            measured_ttft_count: self.measured_ttft_count.load(Ordering::Relaxed),
            cost_micros: self.cost_micros.load(Ordering::Relaxed),
        };

        let epoch2 = self.epoch.load(Ordering::Acquire);
        if epoch1 == epoch2 {
            Some(snap)
        } else {
            // 读取期间被覆写，跳过该槽
            None
        }
    }

    fn add_delta(&self, delta: &MetricDelta) {
        self.request_count
            .fetch_add(delta.request_count, Ordering::Relaxed);
        self.success_count
            .fetch_add(delta.success_count, Ordering::Relaxed);
        self.failure_count
            .fetch_add(delta.failure_count, Ordering::Relaxed);
        self.total_input_tokens
            .fetch_add(delta.total_input_tokens, Ordering::Relaxed);
        self.cache_read_input_tokens
            .fetch_add(delta.cache_read_input_tokens, Ordering::Relaxed);
        self.cache_write_input_tokens
            .fetch_add(delta.cache_write_input_tokens, Ordering::Relaxed);
        self.output_tokens
            .fetch_add(delta.output_tokens, Ordering::Relaxed);
        self.reasoning_output_tokens
            .fetch_add(delta.reasoning_output_tokens, Ordering::Relaxed);
        self.provider_duration_ms
            .fetch_add(delta.provider_duration_ms, Ordering::Relaxed);
        self.provider_calls
            .fetch_add(delta.provider_calls, Ordering::Relaxed);
        self.ttft_duration_ms
            .fetch_add(delta.ttft_duration_ms, Ordering::Relaxed);
        self.measured_ttft_count
            .fetch_add(delta.measured_ttft_count, Ordering::Relaxed);
        self.cost_micros
            .fetch_add(delta.cost_micros, Ordering::Relaxed);
    }

    fn reset_and_set(&self, delta: &MetricDelta) {
        self.request_count
            .store(delta.request_count, Ordering::Relaxed);
        self.success_count
            .store(delta.success_count, Ordering::Relaxed);
        self.failure_count
            .store(delta.failure_count, Ordering::Relaxed);
        self.total_input_tokens
            .store(delta.total_input_tokens, Ordering::Relaxed);
        self.cache_read_input_tokens
            .store(delta.cache_read_input_tokens, Ordering::Relaxed);
        self.cache_write_input_tokens
            .store(delta.cache_write_input_tokens, Ordering::Relaxed);
        self.output_tokens
            .store(delta.output_tokens, Ordering::Relaxed);
        self.reasoning_output_tokens
            .store(delta.reasoning_output_tokens, Ordering::Relaxed);
        self.provider_duration_ms
            .store(delta.provider_duration_ms, Ordering::Relaxed);
        self.provider_calls
            .store(delta.provider_calls, Ordering::Relaxed);
        self.ttft_duration_ms
            .store(delta.ttft_duration_ms, Ordering::Relaxed);
        self.measured_ttft_count
            .store(delta.measured_ttft_count, Ordering::Relaxed);
        self.cost_micros.store(delta.cost_micros, Ordering::Relaxed);
    }
}

use super::observability_realtime::build_realtime_window_report;

/// 内存无锁滑动窗口环形缓冲。
///
/// 固定预分配 360 槽（40KB 内存，远低于 5MB 预算），零堆动态扩展，零互斥锁。
pub struct SlidingWindowRingBuffer {
    slots: Box<[WindowSlot; SLIDING_WINDOW_SLOT_COUNT]>,
}

impl Default for SlidingWindowRingBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl SlidingWindowRingBuffer {
    /// 构造全新的 360 槽滑动窗口。
    #[must_use]
    pub fn new() -> Self {
        let slots: Vec<WindowSlot> = (0..SLIDING_WINDOW_SLOT_COUNT)
            .map(|_| WindowSlot::default())
            .collect();
        let boxed: Box<[WindowSlot]> = slots.into_boxed_slice();
        let array_box = match boxed.try_into() {
            Ok(arr) => arr,
            Err(_) => unreachable!("槽位数量固定为 360"),
        };
        Self { slots: array_box }
    }

    /// 记录单条事实增量。
    pub fn record(&self, timestamp_ms: u64, delta: &MetricDelta) {
        let slot_id = timestamp_ms / SLIDING_WINDOW_SLOT_DURATION_MS;
        let index = (slot_id as usize) % SLIDING_WINDOW_SLOT_COUNT;
        self.slots[index].record(slot_id, delta);
    }

    /// 获取指定窗口时长的聚合快照（window_minutes 典型值为 15, 30, 60）。
    #[must_use]
    pub fn snapshot(&self, now_ms: u64, window_minutes: u32) -> RealtimeWindowReport {
        let window_minutes = window_minutes.clamp(1, 60);
        let current_slot_id = now_ms / SLIDING_WINDOW_SLOT_DURATION_MS;
        let requested_slots = (window_minutes as usize * 6).min(SLIDING_WINDOW_SLOT_COUNT);

        let mut snapshots: Vec<SlotSnapshot> = Vec::with_capacity(requested_slots);
        let start_slot_id = current_slot_id.saturating_sub(requested_slots as u64 - 1);

        for slot_id in start_slot_id..=current_slot_id {
            let index = (slot_id as usize) % SLIDING_WINDOW_SLOT_COUNT;
            let snap = self.slots[index]
                .snapshot_at(slot_id)
                .unwrap_or(SlotSnapshot {
                    slot_id,
                    ..Default::default()
                });
            snapshots.push(snap);
        }

        build_realtime_window_report(window_minutes, current_slot_id, &snapshots)
    }
}
/// 吞吐量时序点（支撑 SSOT §5.2 throughputSeries）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThroughputPoint {
    /// 毫秒时间戳。
    pub timestamp_ms: u64,
    /// 折合每分钟 Token 数。
    pub token_per_min: u64,
    /// 折合每分钟请求数。
    pub request_per_min: u64,
}

/// 请求状态时序点（支撑 SSOT §5.2 requestStatusSeries）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestStatusPoint {
    /// 毫秒时间戳。
    pub timestamp_ms: u64,
    /// 成功请求数。
    pub success_count: u64,
    /// 失败请求数。
    pub failure_count: u64,
    /// 失败率（0.0 ~ 1.0）。
    pub failure_rate: f64,
}

/// Token 构成明细（支撑 SSOT §5.2 tokenComposition）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenComposition {
    /// 未命中缓存输入 token。
    pub uncached_input_tokens: u64,
    /// 命中缓存读取 token。
    pub cache_read_tokens: u64,
    /// 缓存写入 token。
    pub cache_write_tokens: u64,
    /// 输出 token。
    pub output_tokens: u64,
    /// 思考 token。
    pub reasoning_tokens: u64,
}

/// 聚合核心指标（支撑 SSOT §5.2 metrics）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeSummaryMetrics {
    /// 当前每分钟 Token 数（最近 1 分钟）。
    pub current_tpm: u64,
    /// 当前每分钟请求数（最近 1 分钟）。
    pub current_rpm: u64,
    /// 窗口平均每分钟 Token 数。
    pub average_tpm: f64,
    /// 窗口平均每分钟请求数。
    pub average_rpm: f64,
}

/// 实时监控窗口报告（直接对齐 SSOT §5.2 响应契约）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RealtimeWindowReport {
    /// 窗口标识（如 "15m", "30m", "60m"）。
    pub window: String,
    /// 窗口内请求总数。
    pub window_request_count: u64,
    /// 窗口内 Token 总数。
    pub window_total_tokens: u64,
    /// 窗口缓存命中率基点（10000 进制）。
    pub window_cache_hit_rate_basis_points: u16,
    /// 窗口总计费微美元。
    pub window_cost_micros: u64,
    /// 核心度量摘要。
    pub metrics: RealtimeSummaryMetrics,
    /// 吞吐量时序点序列。
    pub throughput_series: Vec<ThroughputPoint>,
    /// 状态时序点序列。
    pub request_status_series: Vec<RequestStatusPoint>,
    /// Token 构成明细。
    pub token_composition: TokenComposition,
    /// 最新首字延迟（毫秒）。
    pub ttft_current_ms: u64,
    /// 窗口平均首字延迟（毫秒）。
    pub ttft_average_ms: u64,
    /// 最新提供方延迟（毫秒）。
    pub latency_current_ms: u64,
    /// 窗口平均提供方延迟（毫秒）。
    pub latency_average_ms: u64,
}

// ============================================================================
// GitHub 风格活动矩阵热力图分桶算法（纯函数）
// ============================================================================

/// 热力图分桶数据项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeatmapBucket {
    /// 本地日期标识（"YYYY-MM-DD"）。
    pub date: String,
    /// 活跃度等级（0~4 档）。
    pub level: u8,
    /// 实际度量值。
    pub value: u64,
}

/// GitHub 风格 5 档热力图分级纯函数。
///
/// - 0 档：value == 0 或 max_value == 0
/// - 1~4 档：按与最大值的四分位阈值划分 (0, 0.25] -> 1, (0.25, 0.50] -> 2, (0.50, 0.75] -> 3, (0.75, 1.0] -> 4
#[must_use]
pub fn calculate_heatmap_level(value: u64, max_value: u64) -> u8 {
    if value == 0 || max_value == 0 {
        0
    } else {
        let ratio = value as f64 / max_value as f64;
        if ratio <= 0.25 {
            1
        } else if ratio <= 0.50 {
            2
        } else if ratio <= 0.75 {
            3
        } else {
            4
        }
    }
}

/// 批量构造热力图桶。
#[must_use]
pub fn build_activity_heatmap_buckets(daily_values: &[(String, u64)]) -> Vec<HeatmapBucket> {
    let max_value = daily_values.iter().map(|(_, v)| *v).max().unwrap_or(0);
    daily_values
        .iter()
        .map(|(date, value)| HeatmapBucket {
            date: date.clone(),
            level: calculate_heatmap_level(*value, max_value),
            value: *value,
        })
        .collect()
}

// ============================================================================
// 请求健康度 3 档分类算法（纯函数）
// ============================================================================

/// 请求健康度状态三档（healthy / degraded / failed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    /// 全部成功。
    Healthy,
    /// 部分失败。
    Degraded,
    /// 全部失败。
    Failed,
}

/// 健康度时间线桶项。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthTimelineBucket {
    /// 毫秒时间戳。
    pub timestamp_ms: u64,
    /// 健康度等级。
    pub status: HealthStatus,
    /// 成功率（0.0 ~ 1.0）。
    pub success_rate: f64,
}

/// 请求健康度分类纯函数。
///
/// 规则：
/// - total_requests == 0：无请求的桶不产出（返回 None）；
/// - failure_count == 0：healthy；
/// - 0 < failure_count < total_requests：degraded；
/// - failure_count == total_requests：failed。
#[must_use]
pub fn classify_health(total_requests: u64, failure_count: u64) -> Option<(HealthStatus, f64)> {
    if total_requests == 0 {
        return None;
    }
    let success_count = total_requests.saturating_sub(failure_count);
    let success_rate = success_count as f64 / total_requests as f64;
    let status = if failure_count == 0 {
        HealthStatus::Healthy
    } else if failure_count < total_requests {
        HealthStatus::Degraded
    } else {
        HealthStatus::Failed
    };
    Some((status, success_rate))
}

#[cfg(test)]
#[path = "observability_tests.rs"]
mod observability_tests;
