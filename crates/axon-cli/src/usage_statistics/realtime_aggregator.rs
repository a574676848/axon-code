//! 实时指标聚合器单例与运行时适配。
//!
//! 负责：
//! 1. 驻留进程级全局无锁滑动窗口 (`SlidingWindowRingBuffer`)；
//! 2. 将落库事实 (`UsageFact`) 转换为 `MetricDelta` 并累加；
//! 3. 输出快照供诊断与 API 使用。

use std::sync::OnceLock;

use axon_domain::{MetricDelta, RealtimeWindowReport, SlidingWindowRingBuffer};

use super::model::UsageFact;

static GLOBAL_SLIDING_WINDOW: OnceLock<SlidingWindowRingBuffer> = OnceLock::new();

/// 获取进程级全局无锁滑动窗口引用。
#[must_use]
pub fn global_sliding_window() -> &'static SlidingWindowRingBuffer {
    GLOBAL_SLIDING_WINDOW.get_or_init(SlidingWindowRingBuffer::new)
}

/// 将持久化事实转换为指标增量模型。
#[must_use]
pub(super) fn fact_to_metric_delta(fact: &UsageFact) -> MetricDelta {
    MetricDelta {
        request_count: 1,
        success_count: if fact.status.as_str() == "completed" {
            1
        } else {
            0
        },
        failure_count: if fact.status.as_str() == "failed" {
            1
        } else {
            0
        },
        total_input_tokens: fact.total_input_tokens,
        cache_read_input_tokens: fact.cache_read_input_tokens,
        cache_write_input_tokens: fact.cache_write_input_tokens,
        output_tokens: fact.output_tokens,
        reasoning_output_tokens: fact.reasoning_output_tokens,
        provider_duration_ms: fact.provider_duration_ms,
        provider_calls: fact.provider_calls,
        ttft_duration_ms: fact.ttft_ms.unwrap_or(0),
        measured_ttft_count: if fact.ttft_ms.is_some() { 1 } else { 0 },
        cost_micros: fact.cost_micros.unwrap_or(0),
    }
}

/// 将已判定实际变更的持久化事实累加至内存滑动窗口。
pub(super) fn record_fact_to_sliding_window(fact: &UsageFact) {
    let delta = fact_to_metric_delta(fact);
    global_sliding_window().record(fact.recorded_at_ms, &delta);
}

/// 获取全局滑动窗口快照。
#[must_use]
pub fn sliding_window_snapshot(now_ms: u64, window_minutes: u32) -> RealtimeWindowReport {
    global_sliding_window().snapshot(now_ms, window_minutes)
}
