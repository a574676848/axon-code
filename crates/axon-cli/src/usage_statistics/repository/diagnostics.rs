//! 深度诊断计算器（SSOT §5.4 / §2.1 - M4）。
//!
//! 1. 分位数计算器：nearest-rank 算法（输出 p50/p90/p95/p99）；
//! 2. 样本上限与等距抽稀（≤ 7 天，≤ 10,000 条原始样本，触限披露抽稀率）；
//! 3. 散点上限（≤ 2,000 点，等距抽稀）；
//! 4. 模型效益分析（USD / 1M tokens，4 位小数，total_tokens=0 不进榜）。

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::Connection;

use super::super::observability_model::{
    DiagnosticsPercentiles, DiagnosticsReport, ModelEfficiencyItem, SamplingDisclosure,
    ScatterPoint,
};
use super::{db_error, from_i64, to_i64};

/// 诊断计算原始样本最大上限（防止全表扫描与内存膨胀）。
pub const DIAGNOSTICS_MAX_RAW_SAMPLES: usize = 10_000;
/// 散点数量最大上限。
pub const DIAGNOSTICS_MAX_SCATTER_POINTS: usize = 2_000;
/// 诊断时间窗口最大上限（7 天毫秒数）。
pub const DIAGNOSTICS_MAX_WINDOW_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

/// 原始诊断事实行数据。
#[derive(Debug, Clone)]
pub struct RawDiagnosticFact {
    pub recorded_at_ms: u64,
    pub model: String,
    pub ttft_ms: Option<u64>,
    pub provider_duration_ms: u64,
    pub total_tokens: u64,
    pub cost_micros: u64,
}

/// 计算 nearest-rank 百分位数（纯函数）。
///
/// 算法遵循 SSOT §2.1 与 `first_token_telemetry.rs`：
/// `rank = ceil(N * P / 100) = (N * P + 99) / 100`。
#[must_use]
pub fn nearest_rank_percentile(sorted_samples: &[u64], percentile: usize) -> Option<u64> {
    if sorted_samples.is_empty() || percentile == 0 || percentile > 100 {
        return None;
    }
    let rank = sorted_samples
        .len()
        .saturating_mul(percentile)
        .saturating_add(99)
        / 100;
    sorted_samples.get(rank.saturating_sub(1)).copied()
}

/// 样本等距抽稀算法（按原有序序列挑选样本，保留首尾边界）。
pub fn thin_equidistant<T>(
    mut items: Vec<T>,
    limit: usize,
) -> (Vec<T>, Option<SamplingDisclosure>) {
    let total = items.len();
    if total <= limit {
        return (items, None);
    }
    if limit == 0 {
        return (
            Vec::new(),
            Some(SamplingDisclosure {
                sampled_of: total,
                sample_count: 0,
                sampling_ratio: 0.0,
            }),
        );
    }
    if limit == 1 {
        let first = items.remove(0);
        return (
            vec![first],
            Some(SamplingDisclosure {
                sampled_of: total,
                sample_count: 1,
                sampling_ratio: (1.0 / total as f64 * 10_000.0).round() / 10_000.0,
            }),
        );
    }

    let step = (total - 1) as f64 / (limit - 1) as f64;
    let mut selected_indices = BTreeSet::new();
    for i in 0..limit {
        let idx = ((i as f64) * step).round() as usize;
        selected_indices.insert(idx.min(total - 1));
    }

    let mut result = Vec::with_capacity(selected_indices.len());
    for (i, item) in items.into_iter().enumerate() {
        if selected_indices.contains(&i) {
            result.push(item);
        }
    }

    let ratio = result.len() as f64 / total as f64;
    let sampling = SamplingDisclosure {
        sampled_of: total,
        sample_count: result.len(),
        sampling_ratio: (ratio * 10_000.0).round() / 10_000.0,
    };
    (result, Some(sampling))
}

/// 从 SQLite 查询原始诊断样本并执行聚合计算。
pub fn query_diagnostics(
    connection: &Connection,
    from_ms: u64,
    to_ms: u64,
) -> Result<DiagnosticsReport, String> {
    if to_ms < from_ms {
        return Err("诊断时间范围无效：to_ms 小于 from_ms".to_owned());
    }
    let duration = to_ms.saturating_sub(from_ms);
    if duration > DIAGNOSTICS_MAX_WINDOW_MS {
        return Err(format!(
            "诊断窗口超出上限：最大允许 7 天 ({}ms)，当前请求为 {}ms",
            DIAGNOSTICS_MAX_WINDOW_MS, duration
        ));
    }

    let mut stmt = connection
        .prepare(
            "SELECT recorded_at_ms, model, ttft_ms, provider_duration_ms,
                    total_input_tokens + output_tokens AS total_tokens,
                    COALESCE(cost_micros, 0) AS cost_micros
             FROM usage_facts
             WHERE recorded_at_ms >= ?1 AND recorded_at_ms <= ?2
             ORDER BY recorded_at_ms ASC",
        )
        .map_err(db_error("准备 usage_facts 诊断数据查询"))?;

    let rows = stmt
        .query_map([to_i64(from_ms)?, to_i64(to_ms)?], |row| {
            let recorded_at_ms = from_i64(row.get(0)?)?;
            let model: String = row.get(1)?;
            let ttft_raw: Option<i64> = row.get(2)?;
            let ttft_ms = match ttft_raw {
                Some(v) => Some(from_i64(v)?),
                None => None,
            };
            let provider_duration_ms = from_i64(row.get(3)?)?;
            let total_tokens = from_i64(row.get(4)?)?;
            let cost_micros = from_i64(row.get(5)?)?;
            Ok(RawDiagnosticFact {
                recorded_at_ms,
                model,
                ttft_ms,
                provider_duration_ms,
                total_tokens,
                cost_micros,
            })
        })
        .map_err(db_error("执行 usage_facts 诊断数据查询"))?;

    let mut raw_facts = Vec::new();
    for row in rows {
        raw_facts.push(row.map_err(db_error("读取诊断数据行"))?);
    }

    let (facts, sampling) = thin_equidistant(raw_facts, DIAGNOSTICS_MAX_RAW_SAMPLES);
    let report = compute_diagnostics_report(facts, sampling);
    Ok(report)
}

/// 聚合计算诊断报告结构。
#[must_use]
pub fn compute_diagnostics_report(
    facts: Vec<RawDiagnosticFact>,
    sampling: Option<SamplingDisclosure>,
) -> DiagnosticsReport {
    let sample_count = facts.len();

    let mut ttft_samples = Vec::new();
    let mut latency_samples = Vec::new();
    let mut scatter_points = Vec::with_capacity(sample_count.min(DIAGNOSTICS_MAX_SCATTER_POINTS));
    let mut model_stats: BTreeMap<String, (u64, u64)> = BTreeMap::new(); // (total_cost_micros, total_tokens)

    for fact in &facts {
        let _ = fact.recorded_at_ms;
        if let Some(ttft) = fact.ttft_ms {
            ttft_samples.push(ttft);
        }
        latency_samples.push(fact.provider_duration_ms);

        scatter_points.push(ScatterPoint {
            ttft_ms: fact.ttft_ms.unwrap_or(0),
            total_duration_ms: fact.provider_duration_ms,
            tokens: fact.total_tokens,
        });

        let entry = model_stats.entry(fact.model.clone()).or_insert((0, 0));
        entry.0 = entry.0.saturating_add(fact.cost_micros);
        entry.1 = entry.1.saturating_add(fact.total_tokens);
    }

    ttft_samples.sort_unstable();
    latency_samples.sort_unstable();

    let percentiles = DiagnosticsPercentiles {
        p50_ttft_ms: nearest_rank_percentile(&ttft_samples, 50),
        p90_ttft_ms: nearest_rank_percentile(&ttft_samples, 90),
        p95_ttft_ms: nearest_rank_percentile(&ttft_samples, 95),
        p99_ttft_ms: nearest_rank_percentile(&ttft_samples, 99),
        p50_latency_ms: nearest_rank_percentile(&latency_samples, 50),
        p90_latency_ms: nearest_rank_percentile(&latency_samples, 90),
        p95_latency_ms: nearest_rank_percentile(&latency_samples, 95),
        p99_latency_ms: nearest_rank_percentile(&latency_samples, 99),
    };

    let (thinned_scatter, _) = thin_equidistant(scatter_points, DIAGNOSTICS_MAX_SCATTER_POINTS);

    let mut model_efficiency = Vec::new();
    for (model, (cost_micros, tokens)) in model_stats {
        if tokens == 0 {
            // total_tokens = 0 时该模型不进榜 (SSOT §2.1)
            continue;
        }
        let cost_per1m = (cost_micros as f64) / (tokens as f64);
        let rounded = (cost_per1m * 10_000.0).round() / 10_000.0;
        model_efficiency.push(ModelEfficiencyItem {
            model,
            cost_per1m_tokens: rounded,
        });
    }
    model_efficiency.sort_by(|a, b| {
        a.cost_per1m_tokens
            .partial_cmp(&b.cost_per1m_tokens)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    DiagnosticsReport {
        sample_count,
        percentiles,
        scatter_points: thinned_scatter,
        model_efficiency,
        sampling,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nearest_rank_percentile() {
        let samples: Vec<u64> = (1..=100).collect();
        assert_eq!(nearest_rank_percentile(&samples, 50), Some(50));
        assert_eq!(nearest_rank_percentile(&samples, 90), Some(90));
        assert_eq!(nearest_rank_percentile(&samples, 95), Some(95));
        assert_eq!(nearest_rank_percentile(&samples, 99), Some(99));

        let single = vec![42];
        assert_eq!(nearest_rank_percentile(&single, 50), Some(42));
        assert_eq!(nearest_rank_percentile(&single, 99), Some(42));

        let empty: Vec<u64> = vec![];
        assert_eq!(nearest_rank_percentile(&empty, 50), None);
    }

    #[test]
    fn test_thin_equidistant_and_disclosure() {
        let items: Vec<usize> = (0..20_000).collect();
        let (thinned, sampling) = thin_equidistant(items, 10_000);
        assert_eq!(thinned.len(), 10_000);
        assert_eq!(thinned[0], 0);
        assert_eq!(thinned[9_999], 19_999);
        assert!(sampling.is_some());
        let info = sampling.unwrap();
        assert_eq!(info.sampled_of, 20_000);
        assert_eq!(info.sample_count, 10_000);
        assert!((info.sampling_ratio - 0.5).abs() < 1e-4);

        // Limit not reached
        let items_small: Vec<usize> = (0..50).collect();
        let (thinned_small, sampling_small) = thin_equidistant(items_small, 100);
        assert_eq!(thinned_small.len(), 50);
        assert!(sampling_small.is_none());
    }

    #[test]
    fn test_model_efficiency_calculation() {
        let facts = vec![
            RawDiagnosticFact {
                recorded_at_ms: 1000,
                model: "gemini-3.8-flash-high".to_owned(),
                ttft_ms: Some(6800),
                provider_duration_ms: 7380,
                total_tokens: 123_780_000,
                cost_micros: 10_550_000,
            },
            RawDiagnosticFact {
                recorded_at_ms: 2000,
                model: "zero-token-model".to_owned(),
                ttft_ms: None,
                provider_duration_ms: 100,
                total_tokens: 0,
                cost_micros: 0,
            },
        ];
        let report = compute_diagnostics_report(facts, None);
        assert_eq!(report.sample_count, 2);
        assert_eq!(report.model_efficiency.len(), 1);
        assert_eq!(report.model_efficiency[0].model, "gemini-3.8-flash-high");
        assert_eq!(report.model_efficiency[0].cost_per1m_tokens, 0.0852);
    }
}
