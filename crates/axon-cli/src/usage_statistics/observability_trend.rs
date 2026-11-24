//! 小时桶聚合：把按 provider / model / source / status 分行的小时桶并回时间点。
//!
//! `usage_rollup_hour` 的主键含 provider、model、source 与 status，同一小时因此会
//! 返回多行。趋势线、热度日历与健康时间线都以「一个小时一个点」为口径，直接按行
//! 展开会把同一时刻画成多个点，健康度也会退化成「最后一行说了算」。
//!
//! 失败口径与大盘汇总严格同源：只有 `completed` 计入成功，其余终态都计入失败。
//! 这里刻意不使用 `unpriced_request_count`，它是「有没有配到价格」的口径，
//! 与请求成败无关。

use std::collections::BTreeMap;

use super::model::UsageRollupRow;

/// 单个小时桶的合并结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct HourlyPoint {
    /// 该小时的全部请求数。
    pub(super) request_count: u64,
    /// 该小时未成功（非 `completed`）的请求数。
    pub(super) failure_count: u64,
    /// 未命中缓存的输入 Token。
    pub(super) total_input_tokens: u64,
    /// 缓存命中读取的 Token。
    pub(super) cache_read_input_tokens: u64,
    /// 写入缓存的 Token。
    pub(super) cache_write_input_tokens: u64,
    /// 生成输出 Token。
    pub(super) output_tokens: u64,
    /// 推理输出 Token。
    pub(super) reasoning_output_tokens: u64,
    /// 已定价的成本（微美元）。
    pub(super) priced_cost_micros: u64,
}

impl HourlyPoint {
    /// 该小时的 Token 总量：输入、输出与推理之和，与大盘汇总同口径。
    #[must_use]
    pub(super) fn total_tokens(&self) -> u64 {
        self.total_input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.reasoning_output_tokens)
    }
}

/// 按小时桶合并聚合行，返回按时间升序排列的 `(bucket_start_ms, 合并结果)`。
#[must_use]
pub(super) fn merge_hourly_rollups(rows: &[(u64, UsageRollupRow)]) -> Vec<(u64, HourlyPoint)> {
    let mut merged: BTreeMap<u64, HourlyPoint> = BTreeMap::new();
    for (bucket_ms, row) in rows {
        let point = merged.entry(*bucket_ms).or_default();
        point.request_count = point.request_count.saturating_add(row.request_count);
        if row.status != "completed" {
            point.failure_count = point.failure_count.saturating_add(row.request_count);
        }
        point.total_input_tokens = point
            .total_input_tokens
            .saturating_add(row.total_input_tokens);
        point.cache_read_input_tokens = point
            .cache_read_input_tokens
            .saturating_add(row.cache_read_input_tokens);
        point.cache_write_input_tokens = point
            .cache_write_input_tokens
            .saturating_add(row.cache_write_input_tokens);
        point.output_tokens = point.output_tokens.saturating_add(row.output_tokens);
        point.reasoning_output_tokens = point
            .reasoning_output_tokens
            .saturating_add(row.reasoning_output_tokens);
        point.priced_cost_micros = point
            .priced_cost_micros
            .saturating_add(row.priced_cost_micros);
    }
    merged.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, requests: u64, input: u64, output: u64) -> UsageRollupRow {
        UsageRollupRow {
            provider: "p".to_owned(),
            model: "m".to_owned(),
            source: "axon_cli".to_owned(),
            status: status.to_owned(),
            request_count: requests,
            provider_call_count: requests,
            total_input_tokens: input,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            output_tokens: output,
            reasoning_output_tokens: 0,
            provider_duration_ms: 0,
            measured_latency_count: 0,
            priced_cost_micros: 0,
            unpriced_request_count: requests,
        }
    }

    #[test]
    fn same_bucket_rows_merge_into_one_point() {
        let rows = vec![
            (1_000, row("completed", 3, 100, 10)),
            (1_000, row("failed", 2, 50, 0)),
            (2_000, row("completed", 1, 7, 1)),
        ];
        let merged = merge_hourly_rollups(&rows);
        assert_eq!(merged.len(), 2, "同一小时只能产出一个时间点");
        let (bucket, first) = merged[0];
        assert_eq!(bucket, 1_000);
        assert_eq!(first.request_count, 5);
        assert_eq!(first.failure_count, 2, "失败只认非 completed 的终态");
        assert_eq!(first.total_tokens(), 160);
        assert_eq!(merged[1].1.request_count, 1);
    }

    #[test]
    fn unpriced_requests_never_count_as_failures() {
        let rows = vec![(1_000, row("completed", 4, 0, 0))];
        let merged = merge_hourly_rollups(&rows);
        assert_eq!(merged[0].1.failure_count, 0);
        assert_eq!(merged[0].1.request_count, 4);
    }
}
