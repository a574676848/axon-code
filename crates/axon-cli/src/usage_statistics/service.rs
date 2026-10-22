use std::{collections::BTreeMap, fs, path::Path};

use super::{
    model::{
        ModelUsageView, ProviderUsageView, UsageDashboardReport, UsageFilters, UsageMetrics,
        UsagePeriod, UsageProjectionView, UsageRollupRow, UsageTrendPoint,
    },
    projection::period_start_ms,
    repository::{UsageQuery, UsageRepository},
};

const USAGE_DASHBOARD_SCHEMA_VERSION: u16 = 1;
const COST_STATUS_PRICED: &str = "priced";
const COST_STATUS_PARTIAL: &str = "partial";
const COST_STATUS_UNPRICED: &str = "unpriced";

#[derive(Debug, Default)]
struct MetricsAccumulator {
    request_count: u64,
    provider_call_count: u64,
    successful_request_count: u64,
    cache_read_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
    reasoning_output_tokens: u64,
    provider_duration_ms: u64,
    measured_latency_count: u64,
    priced_cost_micros: u64,
    unpriced_request_count: u64,
    total_input_tokens: u64,
}

impl MetricsAccumulator {
    fn add(&mut self, row: &UsageRollupRow) {
        self.request_count = self.request_count.saturating_add(row.request_count);
        self.provider_call_count = self
            .provider_call_count
            .saturating_add(row.provider_call_count);
        if row.status == "completed" {
            self.successful_request_count = self
                .successful_request_count
                .saturating_add(row.request_count);
        }
        self.total_input_tokens = self
            .total_input_tokens
            .saturating_add(row.total_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(row.cache_read_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(row.cache_write_input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(row.output_tokens);
        self.reasoning_output_tokens = self
            .reasoning_output_tokens
            .saturating_add(row.reasoning_output_tokens);
        self.provider_duration_ms = self
            .provider_duration_ms
            .saturating_add(row.provider_duration_ms);
        self.measured_latency_count = self
            .measured_latency_count
            .saturating_add(row.measured_latency_count);
        self.priced_cost_micros = self
            .priced_cost_micros
            .saturating_add(row.priced_cost_micros);
        self.unpriced_request_count = self
            .unpriced_request_count
            .saturating_add(row.unpriced_request_count);
    }

    fn finish(self) -> UsageMetrics {
        let total_tokens = self
            .total_input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.reasoning_output_tokens);
        let cache_denominator = self.total_input_tokens;
        UsageMetrics {
            request_count: self.request_count,
            provider_call_count: self.provider_call_count,
            successful_request_count: self.successful_request_count,
            total_input_tokens: self.total_input_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            cache_write_input_tokens: self.cache_write_input_tokens,
            output_tokens: self.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens,
            total_tokens,
            cache_hit_rate_basis_points: axon_domain::ratio_basis_points(
                self.cache_read_input_tokens,
                cache_denominator,
            ),
            success_rate_basis_points: axon_domain::ratio_basis_points(
                self.successful_request_count,
                self.request_count,
            ),
            average_provider_latency_ms: average(
                self.provider_duration_ms,
                self.measured_latency_count,
            ),
            priced_cost_micros: self.priced_cost_micros,
            unpriced_request_count: self.unpriced_request_count,
            cost_status: cost_status(self.request_count, self.unpriced_request_count),
        }
    }
}

pub(super) struct UsageDashboardQuery<'a> {
    pub(super) period: UsagePeriod,
    pub(super) provider: Option<&'a str>,
    pub(super) model: Option<&'a str>,
    pub(super) source: super::model::UsageSourceFilter,
    pub(super) session_id: Option<&'a str>,
    pub(super) run_id: Option<&'a str>,
    pub(super) recent_limit: usize,
}

pub(super) fn query_dashboard(
    config_path: &Path,
    query: &UsageDashboardQuery<'_>,
) -> Result<UsageDashboardReport, String> {
    let generated_at_ms = axon_time::unix_now_ms();
    let period_start_ms = period_start_ms(query.period, generated_at_ms)?;
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let repository = UsageRepository::open(&axon_root)?;
    let repository_query = UsageQuery {
        period_start_ms,
        provider: query.provider,
        model: query.model,
        source: query.source,
        session_id: query.session_id,
        run_id: query.run_id,
    };
    let rollups = repository.rollups(&repository_query)?;
    let hourly = repository.hourly_rollups(&repository_query)?;
    let recent_requests = repository.recent_requests(&repository_query, query.recent_limit)?;
    let projection = repository.projection_state()?;
    let rollout_path =
        crate::run_commands::resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    let stale = source_modified_ms(&rollout_path)?
        .zip(projection.last_success_at_ms)
        .is_some_and(|(source, projected)| source > projected);

    Ok(UsageDashboardReport {
        schema_version: USAGE_DASHBOARD_SCHEMA_VERSION,
        generated_at_ms,
        period: query.period,
        period_start_ms,
        filters: UsageFilters {
            provider: query.provider.map(ToOwned::to_owned),
            model: query.model.map(ToOwned::to_owned),
            source: query.source,
            session_id: query.session_id.map(ToOwned::to_owned),
            run_id: query.run_id.map(ToOwned::to_owned),
        },
        summary: aggregate_rows(&rollups),
        providers: provider_views(&rollups),
        models: model_views(&rollups),
        trend: trend_views(hourly),
        recent_requests,
        projection: UsageProjectionView {
            last_success_at_ms: projection.last_success_at_ms,
            source_watermark_rowid: projection.source_watermark_rowid,
            projected_fact_count: projection.projected_fact_count,
            stale,
        },
    })
}

fn provider_views(rows: &[UsageRollupRow]) -> Vec<ProviderUsageView> {
    let mut grouped = BTreeMap::<String, MetricsAccumulator>::new();
    for row in rows {
        grouped.entry(row.provider.clone()).or_default().add(row);
    }
    let mut views = grouped
        .into_iter()
        .map(|(provider, metrics)| ProviderUsageView {
            provider,
            metrics: metrics.finish(),
        })
        .collect::<Vec<_>>();
    views.sort_by(|left, right| {
        right
            .metrics
            .total_tokens
            .cmp(&left.metrics.total_tokens)
            .then_with(|| left.provider.cmp(&right.provider))
    });
    views
}

fn model_views(rows: &[UsageRollupRow]) -> Vec<ModelUsageView> {
    let mut grouped = BTreeMap::<String, MetricsAccumulator>::new();
    for row in rows {
        grouped.entry(row.model.clone()).or_default().add(row);
    }
    let mut views = grouped
        .into_iter()
        .map(|(model, metrics)| ModelUsageView {
            model,
            metrics: metrics.finish(),
        })
        .collect::<Vec<_>>();
    views.sort_by(|left, right| {
        right
            .metrics
            .total_tokens
            .cmp(&left.metrics.total_tokens)
            .then_with(|| left.model.cmp(&right.model))
    });
    views
}

fn trend_views(rows: Vec<(u64, UsageRollupRow)>) -> Vec<UsageTrendPoint> {
    let mut grouped = BTreeMap::<u64, MetricsAccumulator>::new();
    for (bucket, row) in rows {
        grouped.entry(bucket).or_default().add(&row);
    }
    grouped
        .into_iter()
        .map(|(bucket_start_ms, metrics)| UsageTrendPoint {
            bucket_start_ms,
            metrics: metrics.finish(),
        })
        .collect()
}

fn aggregate_rows(rows: &[UsageRollupRow]) -> UsageMetrics {
    let mut accumulator = MetricsAccumulator::default();
    for row in rows {
        accumulator.add(row);
    }
    accumulator.finish()
}

fn average(total: u64, count: u64) -> u64 {
    total.checked_div(count).unwrap_or(0)
}

fn cost_status(request_count: u64, unpriced_request_count: u64) -> &'static str {
    if request_count > 0 && unpriced_request_count == 0 {
        COST_STATUS_PRICED
    } else if unpriced_request_count < request_count {
        COST_STATUS_PARTIAL
    } else {
        COST_STATUS_UNPRICED
    }
}

fn source_modified_ms(path: &Path) -> Result<Option<u64>, String> {
    let wal_path = path.with_file_name(format!(
        "{}-wal",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
    ));
    [path, wal_path.as_path()]
        .into_iter()
        .filter(|candidate| candidate.exists())
        .map(|candidate| {
            let modified = fs::metadata(candidate)
                .and_then(|metadata| metadata.modified())
                .map_err(|error| format!("读取 rollout 更新时间失败：{error}"))?;
            modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(axon_time::duration_ms)
                .map_err(|error| format!("转换 rollout 更新时间失败：{error}"))
        })
        .try_fold(None, |latest, modified| {
            modified.map(|modified| Some(latest.map_or(modified, |value: u64| value.max(modified))))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(status: &str, input: u64, cached: u64) -> UsageRollupRow {
        UsageRollupRow {
            provider: "provider-a".to_owned(),
            model: "model-a".to_owned(),
            source: "axon_cli".to_owned(),
            status: status.to_owned(),
            request_count: 1,
            provider_call_count: 2,
            total_input_tokens: input,
            cache_read_input_tokens: cached,
            cache_write_input_tokens: 0,
            output_tokens: 10,
            reasoning_output_tokens: 5,
            provider_duration_ms: 200,
            measured_latency_count: 1,
            priced_cost_micros: 0,
            unpriced_request_count: 1,
        }
    }

    #[test]
    fn aggregation_preserves_cache_and_success_semantics() {
        let metrics = aggregate_rows(&[row("completed", 1_000, 900), row("failed", 100, 0)]);
        assert_eq!(metrics.request_count, 2);
        assert_eq!(metrics.provider_call_count, 4);
        assert_eq!(metrics.total_tokens, 1_130);
        assert_eq!(metrics.total_input_tokens, 1_100);
        assert_eq!(metrics.cache_hit_rate_basis_points, 8_181);
        assert_eq!(metrics.success_rate_basis_points, 5_000);
        assert_eq!(metrics.average_provider_latency_ms, 200);
        assert_eq!(metrics.cost_status, COST_STATUS_UNPRICED);
    }

    #[test]
    fn aggregation_uses_complete_input_as_cache_hit_denominator() {
        let metrics = aggregate_rows(&[row("completed", 3_000, 2_700)]);

        assert_eq!(metrics.total_input_tokens, 3_000);
        assert_eq!(metrics.cache_read_input_tokens, 2_700);
        assert_eq!(metrics.cache_hit_rate_basis_points, 9_000);
    }

    #[test]
    fn cost_status_accounting_contract() {
        // 1. 全命中：unpriced_count = 0 -> priced
        let mut row_priced = row("completed", 1_000, 0);
        row_priced.priced_cost_micros = 5_000;
        row_priced.unpriced_request_count = 0;
        let metrics_priced = aggregate_rows(&[row_priced.clone()]);
        assert_eq!(metrics_priced.cost_status, COST_STATUS_PRICED);

        // 2. 命中但成本舍入为 0 (Some(0)): priced_cost_micros = 0, unpriced_request_count = 0 -> priced
        let mut row_zero = row("completed", 1, 0);
        row_zero.priced_cost_micros = 0;
        row_zero.unpriced_request_count = 0;
        let metrics_zero = aggregate_rows(&[row_zero]);
        assert_eq!(metrics_zero.cost_status, COST_STATUS_PRICED);

        // 3. 部分命中：1条命中，1条未命中 -> partial
        let row_unpriced = row("completed", 500, 0);
        let metrics_partial = aggregate_rows(&[row_priced, row_unpriced]);
        assert_eq!(metrics_partial.cost_status, COST_STATUS_PARTIAL);

        // 4. 全未命中 -> unpriced
        let metrics_unpriced = aggregate_rows(&[row("completed", 1_000, 0)]);
        assert_eq!(metrics_unpriced.cost_status, COST_STATUS_UNPRICED);
    }
}
