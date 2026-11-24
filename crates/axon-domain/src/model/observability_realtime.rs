//! 实时窗口报告：把 10 秒槽位的滑窗快照彙总成对外的读数与序列。
//!
//! 与环形缓冲本体分开成篇：缓冲只负责「按槽位原子累加」，这里负责把槽位换算成
//! 人能读的量纲（每分钟、分位、构成占比），两件事的改动理由完全不同。
//!
//! ## 序列为什么按自然分钟给
//!
//! 槽位是 10 秒一格，而接口与界面说的都是「每分钟」。此前的做法是把 10 秒槽位的
//! 计数**乘 6** 当作每分钟：那是把没测到的 50 秒也替它算进去的推算，一次长轮次的
//! 全部 Token 又都记在它的结束时刻那一个槽里，于是单点被放大成几十倍的尖峰
//! （实测出现过 1.35 亿 Token/分钟），图形被一个点压平，读起来像「没有数据」。
//!
//! 现在的口径是**实测**：同一个自然分钟内的 6 个槽位相加就是这一分钟的吞吐，
//! 不做任何折算。窗口首尾两桶可能不满一分钟（窗口边界与当前时刻），如实按已有
//! 槽位累加；窗口总量仍以 `windowTotalTokens` 为准。

use super::observability::{
    RealtimeSummaryMetrics, RealtimeWindowReport, RequestStatusPoint,
    SLIDING_WINDOW_SLOT_DURATION_MS, SlotSnapshot, ThroughputPoint, TokenComposition,
};

/// 一个自然分钟包含的槽位数（10 秒槽位 × 6 = 60 秒）。
///
/// 依赖「槽位时长能整除一分钟」这一条：槽位时长若改成不能整除的值（例如 7 秒），
/// 这里必须改成按时间戳分桶，不能再用整数除法。
const SLOTS_PER_MINUTE: u64 = 60_000 / SLIDING_WINDOW_SLOT_DURATION_MS;

/// 一个自然分钟桶的累加结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct MinuteBucket {
    request_count: u64,
    success_count: u64,
    failure_count: u64,
    tokens: u64,
}

impl MinuteBucket {
    fn accumulate(&mut self, snapshot: &SlotSnapshot) {
        self.request_count = self.request_count.saturating_add(snapshot.request_count);
        self.success_count = self.success_count.saturating_add(snapshot.success_count);
        self.failure_count = self.failure_count.saturating_add(snapshot.failure_count);
        self.tokens = self
            .tokens
            .saturating_add(snapshot.total_input_tokens)
            .saturating_add(snapshot.output_tokens)
            .saturating_add(snapshot.reasoning_output_tokens);
    }
}

/// 汇总窗口报告。
pub(super) fn build_realtime_window_report(
    window_minutes: u32,
    current_slot_id: u64,
    snapshots: &[SlotSnapshot],
) -> RealtimeWindowReport {
    let mut total_requests = 0_u64;
    let mut total_success = 0_u64;
    let mut total_failure = 0_u64;
    let mut total_input = 0_u64;
    let mut cache_read = 0_u64;
    let mut cache_write = 0_u64;
    let mut output = 0_u64;
    let mut reasoning = 0_u64;
    let mut total_duration_ms = 0_u64;
    let mut total_calls = 0_u64;
    let mut total_ttft_ms = 0_u64;
    let mut total_ttft_count = 0_u64;
    let mut total_cost = 0_u64;

    let mut current_minute_requests = 0_u64;
    let mut current_minute_tokens = 0_u64;
    let recent_cutoff_slot_id = current_slot_id.saturating_sub(5); // 最近 6 槽 = 60 秒

    let mut latest_slot_ttft: Option<(u64, u64)> = None;
    let mut latest_slot_latency: Option<(u64, u64)> = None;

    // 序列与窗口总量在同一遍里产出：分钟桶只承担序列，总量仍按槽位逐一累加，
    // 因此首尾不满一分钟的桶不会影响窗口读数。
    let mut minute_ids: Vec<u64> = Vec::new();
    let mut minute_buckets: Vec<MinuteBucket> = Vec::new();

    for snap in snapshots {
        let snap_tokens = snap
            .total_input_tokens
            .saturating_add(snap.output_tokens)
            .saturating_add(snap.reasoning_output_tokens);
        total_requests = total_requests.saturating_add(snap.request_count);
        total_success = total_success.saturating_add(snap.success_count);
        total_failure = total_failure.saturating_add(snap.failure_count);
        total_input = total_input.saturating_add(snap.total_input_tokens);
        cache_read = cache_read.saturating_add(snap.cache_read_input_tokens);
        cache_write = cache_write.saturating_add(snap.cache_write_input_tokens);
        output = output.saturating_add(snap.output_tokens);
        reasoning = reasoning.saturating_add(snap.reasoning_output_tokens);
        total_duration_ms = total_duration_ms.saturating_add(snap.provider_duration_ms);
        total_calls = total_calls.saturating_add(snap.provider_calls);
        total_ttft_ms = total_ttft_ms.saturating_add(snap.ttft_duration_ms);
        total_ttft_count = total_ttft_count.saturating_add(snap.measured_ttft_count);
        total_cost = total_cost.saturating_add(snap.cost_micros);

        if snap.slot_id >= recent_cutoff_slot_id {
            current_minute_requests = current_minute_requests.saturating_add(snap.request_count);
            current_minute_tokens = current_minute_tokens.saturating_add(snap_tokens);
        }

        if snap.measured_ttft_count > 0 {
            latest_slot_ttft = Some((snap.ttft_duration_ms, snap.measured_ttft_count));
        }
        if snap.provider_calls > 0 {
            latest_slot_latency = Some((snap.provider_duration_ms, snap.provider_calls));
        }

        let minute_id = snap.slot_id / SLOTS_PER_MINUTE;
        if minute_ids.last() != Some(&minute_id) {
            minute_ids.push(minute_id);
            minute_buckets.push(MinuteBucket::default());
        }
        if let Some(bucket) = minute_buckets.last_mut() {
            bucket.accumulate(snap);
        }
    }

    let mut throughput_series = Vec::with_capacity(minute_buckets.len());
    let mut request_status_series = Vec::with_capacity(minute_buckets.len());
    for (minute_id, bucket) in minute_ids.iter().zip(&minute_buckets) {
        let timestamp_ms = minute_id.saturating_mul(60_000);
        throughput_series.push(ThroughputPoint {
            timestamp_ms,
            token_per_min: bucket.tokens,
            request_per_min: bucket.request_count,
        });
        let failure_rate = if bucket.request_count > 0 {
            bucket.failure_count as f64 / bucket.request_count as f64
        } else {
            0.0
        };
        request_status_series.push(RequestStatusPoint {
            timestamp_ms,
            success_count: bucket.success_count,
            failure_count: bucket.failure_count,
            failure_rate,
        });
    }

    let window_total_tokens = total_input.saturating_add(output).saturating_add(reasoning);
    let uncached_input = total_input
        .saturating_sub(cache_read)
        .saturating_sub(cache_write);

    let cache_hit_rate_basis_points = if total_input > 0 {
        ((cache_read as u128 * 10_000) / total_input as u128) as u16
    } else {
        0
    };

    let ttft_current_ms = latest_slot_ttft
        .map(|(d, c)| d.checked_div(c).unwrap_or(0))
        .unwrap_or(0);
    let ttft_average_ms = total_ttft_ms.checked_div(total_ttft_count).unwrap_or(0);

    let latency_current_ms = latest_slot_latency
        .map(|(d, c)| d.checked_div(c).unwrap_or(0))
        .unwrap_or(0);
    let latency_average_ms = total_duration_ms.checked_div(total_calls).unwrap_or(0);

    RealtimeWindowReport {
        window: format!("{window_minutes}m"),
        window_request_count: total_requests,
        window_total_tokens,
        window_cache_hit_rate_basis_points: cache_hit_rate_basis_points,
        window_cost_micros: total_cost,
        metrics: RealtimeSummaryMetrics {
            current_tpm: current_minute_tokens,
            current_rpm: current_minute_requests,
            average_tpm: window_total_tokens as f64 / window_minutes as f64,
            average_rpm: total_requests as f64 / window_minutes as f64,
        },
        throughput_series,
        request_status_series,
        token_composition: TokenComposition {
            uncached_input_tokens: uncached_input,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            output_tokens: output,
            reasoning_tokens: reasoning,
        },
        ttft_current_ms,
        ttft_average_ms,
        latency_current_ms,
        latency_average_ms,
    }
}

#[cfg(test)]
#[path = "observability_realtime_tests.rs"]
mod observability_realtime_tests;
