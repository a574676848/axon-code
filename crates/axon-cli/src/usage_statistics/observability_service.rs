//! 可观测性服务层实现（SSOT §5.1 ~ §5.6 / §2.1 ~ §2.3）。
//!
//! 整合 facts、rollup、滑动窗口、诊断、审计日志、CSV/JSON 导出与价格管理。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use axon_domain::{
    HealthTimelineBucket, ModelPriceRule, PricingStyle, RealtimeWindowReport,
    build_activity_heatmap_buckets, classify_health,
};

use super::billing::BillingEngine;
use super::model::UsagePeriod;
use super::observability_model::{
    AuditLogsReport, DiagnosticsReport, DistributionItem, DistributionReport,
    DistributionTimeSeriesPoint, HealthTimelineData, HeatmapData, OverviewReport, OverviewSummary,
    PricingAutoSyncReport, RecalculatePricingResponse, SavePricingRulePayload, Sparklines,
    SyncPricingResponse,
};
use super::projection::period_start_ms;
use super::repository::audit::{
    AuditLogQuery, export_audit_logs_csv, export_audit_logs_json, query_audit_logs,
};
use super::repository::diagnostics::query_diagnostics;
use super::repository::{UsageQuery, UsageRepository};
use super::sync::PricingSyncService;

// ============================================================================
// 大盘概览 (GET /api/v1/observability/overview)
// ============================================================================

pub fn query_overview(
    config_path: &Path,
    period_str: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
    source: Option<&str>,
) -> Result<OverviewReport, String> {
    let period = parse_period(period_str.unwrap_or("all"))?;
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let now_ms = axon_time::unix_now_ms();
    let period_start_ms = period_start_ms(period, now_ms)?;

    let source_filter = match source {
        Some("axon_cli") => super::model::UsageSourceFilter::AxonCli,
        Some("subagent") => super::model::UsageSourceFilter::Subagent,
        _ => super::model::UsageSourceFilter::All,
    };

    let repo = UsageRepository::open(&axon_root)?;
    let query = UsageQuery {
        period_start_ms,
        provider,
        model,
        source: source_filter,
        session_id: None,
        run_id: None,
    };

    let rollups = repo.rollups(&query)?;
    let hourly = repo.hourly_rollups(&query)?;

    // 速率读数的分母：固定周期用周期起点，「全部」用第一条事实的时间。
    let rate_start_ms = rate_window_start_ms(period, period_start_ms, &hourly, now_ms);

    let mut request_count = 0_u64;
    let mut success_count = 0_u64;
    let mut failure_count = 0_u64;
    let mut total_input = 0_u64;
    let mut cache_read = 0_u64;
    let mut cache_write = 0_u64;
    let mut output = 0_u64;
    let mut reasoning = 0_u64;
    let mut total_cost_micros = 0_u64;

    for r in &rollups {
        request_count = request_count.saturating_add(r.request_count);
        if r.status == "completed" {
            success_count = success_count.saturating_add(r.request_count);
        } else {
            failure_count = failure_count.saturating_add(r.request_count);
        }
        total_input = total_input.saturating_add(r.total_input_tokens);
        cache_read = cache_read.saturating_add(r.cache_read_input_tokens);
        cache_write = cache_write.saturating_add(r.cache_write_input_tokens);
        output = output.saturating_add(r.output_tokens);
        reasoning = reasoning.saturating_add(r.reasoning_output_tokens);
        total_cost_micros = total_cost_micros.saturating_add(r.priced_cost_micros);
    }

    let total_tokens = total_input.saturating_add(output).saturating_add(reasoning);
    let success_rate_basis_points = if request_count > 0 {
        ((success_count as u128 * 10_000) / request_count as u128) as u16
    } else {
        10_000
    };
    let cache_hit_rate_basis_points = if total_input > 0 {
        ((cache_read as u128 * 10_000) / total_input as u128) as u16
    } else {
        0
    };

    let duration_minutes = now_ms.saturating_sub(rate_start_ms).max(60_000) as f64 / 60_000.0;
    // 速率按原值返回，不在服务端四舍五入：个人用量常在千分之几次的量级，
    // 先round到两位小数会把 0.004 变成 0，界面再想格式化也救不回来。
    let current_rpm = request_count as f64 / duration_minutes;
    let current_tpm = (total_tokens as f64 / duration_minutes).round() as u64;

    let cost_dollars = total_cost_micros as f64 / 1_000_000.0;
    let cost_formatted = format!("${cost_dollars:.2}");

    let summary = OverviewSummary {
        request_count,
        success_count,
        failure_count,
        success_rate_basis_points,
        total_tokens,
        total_input_tokens: total_input,
        cache_read_input_tokens: cache_read,
        cache_write_input_tokens: cache_write,
        output_tokens: output,
        reasoning_output_tokens: reasoning,
        cache_hit_rate_basis_points,
        current_rpm,
        current_tpm,
        total_cost_micros,
        cost_formatted,
    };

    // 小时桶先按时间点合并：同一小时可能按 provider / model / source / status 分成多行，
    // 不合并会把一个时刻画成多个点，健康度也会退化成「最后一行说了算」。
    let hourly = super::observability_trend::merge_hourly_rollups(&hourly);

    // Sparklines 迷你走势 (最近 24 小时或分桶)
    let mut spark_requests = Vec::new();
    let mut spark_tokens = Vec::new();
    let mut spark_costs = Vec::new();
    for (_bucket_ms, point) in hourly.iter().rev().take(24).rev() {
        spark_requests.push(point.request_count);
        spark_tokens.push(point.total_tokens());
        spark_costs
            .push((point.priced_cost_micros as f64 / 1_000_000.0 * 10_000.0).round() / 10_000.0);
    }
    let sparklines = Sparklines {
        requests: spark_requests,
        tokens: spark_tokens,
        costs: spark_costs,
    };

    // Activity Heatmap
    let mut daily_tokens: BTreeMap<String, u64> = BTreeMap::new();
    for (bucket_ms, point) in &hourly {
        let dt = chrono::DateTime::from_timestamp_millis(*bucket_ms as i64)
            .map(|t| t.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        if !dt.is_empty() {
            *daily_tokens.entry(dt).or_default() += point.total_tokens();
        }
    }
    let daily_pairs: Vec<(String, u64)> = daily_tokens.into_iter().collect();
    let activity_heatmap = HeatmapData {
        metric: "tokens".to_owned(),
        buckets: build_activity_heatmap_buckets(&daily_pairs),
    };

    // Health Timeline：失败数取自终态，与大盘 summary 的成败口径完全一致。
    let mut health_buckets = Vec::new();
    for (bucket_ms, point) in &hourly {
        if let Some((status, rate)) = classify_health(point.request_count, point.failure_count) {
            health_buckets.push(HealthTimelineBucket {
                timestamp_ms: *bucket_ms,
                status,
                success_rate: (rate * 100.0).round() / 100.0,
            });
        }
    }
    let health_timeline = HealthTimelineData {
        buckets: health_buckets,
    };

    Ok(OverviewReport {
        summary,
        sparklines,
        activity_heatmap,
        health_timeline,
    })
}

// ============================================================================
// 实时监控 (GET /api/v1/observability/realtime)
// ============================================================================

pub fn query_realtime(
    config_path: &Path,
    window_str: Option<&str>,
) -> Result<RealtimeWindowReport, String> {
    let window_minutes = match window_str.unwrap_or("15m") {
        "15m" | "15" => 15,
        "30m" | "30" => 30,
        "60m" | "60" => 60,
        other => {
            return Err(format!(
                "无效 realtime window 参数: {other}，仅支持 15m, 30m, 60m"
            ));
        }
    };
    let _ = config_path; // 滑动窗口驻留在进程内存中
    let now_ms = axon_time::unix_now_ms();
    Ok(super::realtime_aggregator::sliding_window_snapshot(
        now_ms,
        window_minutes,
    ))
}

// ============================================================================
// 用量分布与排行 (GET /api/v1/observability/distribution)
// ============================================================================

pub fn query_distribution(
    config_path: &Path,
    dimension_str: Option<&str>,
    period_str: Option<&str>,
) -> Result<DistributionReport, String> {
    let dimension = dimension_str.unwrap_or("model");
    if dimension != "model" && dimension != "provider" {
        return Err(format!(
            "无效 distribution dimension: {dimension}，仅支持 model 或 provider"
        ));
    }
    let period = parse_period(period_str.unwrap_or("all"))?;
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let now_ms = axon_time::unix_now_ms();
    let period_start_ms = period_start_ms(period, now_ms)?;

    let repo = UsageRepository::open(&axon_root)?;
    let query = UsageQuery {
        period_start_ms,
        provider: None,
        model: None,
        source: super::model::UsageSourceFilter::All,
        session_id: None,
        run_id: None,
    };

    let rollups = repo.rollups(&query)?;
    let hourly = repo.hourly_rollups(&query)?;

    // 与概览同一口径：速率的分母在「全部」下取第一条事实的时间。
    let rate_start_ms = rate_window_start_ms(period, period_start_ms, &hourly, now_ms);

    let mut total_tokens = 0_u64;
    let mut total_cost_micros = 0_u64;

    struct DimAccum {
        tokens: u64,
        requests: u64,
        cost_micros: u64,
        input_tokens: u64,
        cache_read_tokens: u64,
    }

    let mut dim_map: BTreeMap<String, DimAccum> = BTreeMap::new();
    for r in &rollups {
        let key = if dimension == "provider" {
            r.provider.clone()
        } else {
            r.model.clone()
        };
        let row_tokens = r.total_input_tokens + r.output_tokens + r.reasoning_output_tokens;
        total_tokens = total_tokens.saturating_add(row_tokens);
        total_cost_micros = total_cost_micros.saturating_add(r.priced_cost_micros);

        let entry = dim_map.entry(key).or_insert(DimAccum {
            tokens: 0,
            requests: 0,
            cost_micros: 0,
            input_tokens: 0,
            cache_read_tokens: 0,
        });
        entry.tokens = entry.tokens.saturating_add(row_tokens);
        entry.requests = entry.requests.saturating_add(r.request_count);
        entry.cost_micros = entry.cost_micros.saturating_add(r.priced_cost_micros);
        entry.input_tokens = entry.input_tokens.saturating_add(r.total_input_tokens);
        entry.cache_read_tokens = entry
            .cache_read_tokens
            .saturating_add(r.cache_read_input_tokens);
    }

    let duration_minutes = now_ms.saturating_sub(rate_start_ms).max(60_000) as f64 / 60_000.0;

    let mut items = Vec::new();
    for (key, acc) in dim_map {
        let percentage = if total_tokens > 0 {
            (acc.tokens as f64 / total_tokens as f64 * 10_000.0).round() / 100.0
        } else {
            0.0
        };
        let cache_hit_rate_basis_points = if acc.input_tokens > 0 {
            ((acc.cache_read_tokens as u128 * 10_000) / acc.input_tokens as u128) as u16
        } else {
            0
        };
        // 与概览同一口径：速率按原值返回，由调用方决定怎么显示。
        let rpm = acc.requests as f64 / duration_minutes;
        let tpm = (acc.tokens as f64 / duration_minutes).round() as u64;

        items.push(DistributionItem {
            key,
            tokens: acc.tokens,
            percentage,
            request_count: acc.requests,
            cost_micros: acc.cost_micros,
            cache_hit_rate_basis_points,
            tpm,
            rpm,
        });
    }

    items.sort_by_key(|a| std::cmp::Reverse(a.tokens));

    let time_series: Vec<DistributionTimeSeriesPoint> =
        super::observability_trend::merge_hourly_rollups(&hourly)
            .into_iter()
            .map(|(bucket_ms, h)| {
                let uncached = h
                    .total_input_tokens
                    .saturating_sub(h.cache_read_input_tokens);
                DistributionTimeSeriesPoint {
                    timestamp_ms: bucket_ms,
                    uncached_input_tokens: uncached,
                    cache_read_tokens: h.cache_read_input_tokens,
                    cache_write_tokens: h.cache_write_input_tokens,
                    output_tokens: h.output_tokens,
                    reasoning_tokens: h.reasoning_output_tokens,
                    request_count: h.request_count,
                    cost_micros: h.priced_cost_micros,
                }
            })
            .collect();

    Ok(DistributionReport {
        total_tokens,
        total_cost_micros,
        items,
        time_series,
    })
}

// ============================================================================
// 延迟与效率诊断 (GET /api/v1/observability/diagnostics)
// ============================================================================

pub fn query_diagnostics_service(
    config_path: &Path,
    period_str: Option<&str>,
) -> Result<DiagnosticsReport, String> {
    let period = period_str.unwrap_or("7d");
    if period != "today" && period != "7d" {
        return Err(format!(
            "诊断接口 period 参数无效: {period}。窗口最大允许 7 天 (仅支持 today 或 7d)"
        ));
    }
    let parsed_period = if period == "today" {
        UsagePeriod::Today
    } else {
        UsagePeriod::SevenDays
    };

    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let now_ms = axon_time::unix_now_ms();
    let from_ms = period_start_ms(parsed_period, now_ms)?;

    let repo = UsageRepository::open(&axon_root)?;
    query_diagnostics(repo.connection(), from_ms, now_ms)
}

// ============================================================================
// 审计日志检索与导出 (GET /api/v1/observability/audit/*)
// ============================================================================

pub fn query_audit_logs_service(
    config_path: &Path,
    query: &AuditLogQuery<'_>,
) -> Result<AuditLogsReport, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let repo = UsageRepository::open(&axon_root)?;
    query_audit_logs(repo.connection(), query)
}

pub fn export_audit_logs_service<W: Write>(
    config_path: &Path,
    query: &AuditLogQuery<'_>,
    format_str: &str,
    writer: W,
) -> Result<u64, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let repo = UsageRepository::open(&axon_root)?;
    match format_str.to_ascii_lowercase().as_str() {
        "csv" => export_audit_logs_csv(repo.connection(), query, writer),
        "json" => export_audit_logs_json(repo.connection(), query, writer),
        other => Err(format!("不支持的导出格式: {other}，仅支持 csv 或 json")),
    }
}

// ============================================================================
// 模型价格规则管理 (SSOT §5.6 / §2.3)
// ============================================================================

pub fn list_pricing_rules_service(config_path: &Path) -> Result<Vec<ModelPriceRule>, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let repo = UsageRepository::open(&axon_root)?;
    repo.list_pricing_rules()
}

pub fn save_pricing_rule_service(
    config_path: &Path,
    payload: SavePricingRulePayload,
) -> Result<String, String> {
    let pricing_style = PricingStyle::parse(&payload.pricing_style)
        .ok_or_else(|| format!("未知 pricingStyle: {}", payload.pricing_style))?;
    let now_ms = axon_time::unix_now_ms();
    let id = format!("pr_{}", now_ms);

    let rule = ModelPriceRule {
        id: id.clone(),
        model_pattern: payload.model_pattern,
        pricing_style,
        input_price_per_m_micros: payload.input_price_per_m_micros,
        output_price_per_m_micros: payload.output_price_per_m_micros,
        cache_read_price_per_m_micros: payload.cache_read_price_per_m_micros,
        cache_write_price_per_m_micros: payload.cache_write_price_per_m_micros,
        multiplier_basis_points: payload.multiplier_basis_points,
        priority: payload.priority,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    };

    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let repo = UsageRepository::open(&axon_root)?;
    repo.save_pricing_rule(&rule)?;
    Ok(id)
}

pub fn delete_pricing_rule_service(config_path: &Path, id: &str) -> Result<bool, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let repo = UsageRepository::open(&axon_root)?;
    repo.delete_pricing_rule(id)
}

pub async fn sync_pricing_service(config_path: &Path) -> Result<SyncPricingResponse, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let sync_svc = PricingSyncService::new();
    let outcome = sync_svc.sync(&axon_root, true).await?;
    let now_ms = axon_time::unix_now_ms();
    // 「写入 0 条」有四种完全不同的原因：该等一会儿、该明天再说、还是该去查网络。
    // 因此结论与原因一并返回，调用方不必靠猜。
    let (count, outcome_key, detail) = match outcome {
        super::sync::SyncOutcome::Success { rules_saved } => (rules_saved, "success", None),
        super::sync::SyncOutcome::Debounced { remaining_ms } => (
            0,
            "debounced",
            Some(format!(
                "手动同步的防抖窗口还剩 {} 秒",
                remaining_ms.div_ceil(1000)
            )),
        ),
        super::sync::SyncOutcome::RateLimited => (
            0,
            "rate_limited",
            Some("自动同步每天最多执行一次".to_owned()),
        ),
        super::sync::SyncOutcome::FallbackToExisting { error } => (0, "fallback", Some(error)),
    };
    Ok(SyncPricingResponse {
        synced_rules_count: count,
        source: "models.dev".to_owned(),
        updated_at_ms: now_ms,
        outcome: outcome_key.to_owned(),
        detail,
    })
}

pub async fn recalculate_pricing_service(
    config_path: &Path,
    _from_ms: Option<u64>,
) -> Result<RecalculatePricingResponse, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let billing = BillingEngine::new();
    let report = billing.recalculate(&axon_root).await?;
    Ok(RecalculatePricingResponse {
        recalculated_count: report.updated_count,
        affected_rollup_buckets: report.batches_processed as usize,
        elapsed_ms: report.total_duration_ms,
    })
}

/// 读取费率自动同步状态。
pub fn pricing_auto_sync_state_service(
    config_path: &Path,
) -> Result<PricingAutoSyncReport, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let state = crate::cron_commands::pricing_sync_cron_state(&axon_root)?;
    Ok(auto_sync_report(state))
}

/// 打开或关闭费率自动同步。
///
/// 打开即创建（或重新启用）系统 Cron 任务，每天同步一次；关闭转为暂停并保留历史。
pub fn set_pricing_auto_sync_service(
    config_path: &Path,
    enabled: bool,
) -> Result<PricingAutoSyncReport, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let state = crate::cron_commands::set_pricing_sync_cron_enabled(&axon_root, enabled)?;
    Ok(auto_sync_report(state))
}

fn auto_sync_report(state: crate::cron_commands::PricingSyncCronState) -> PricingAutoSyncReport {
    PricingAutoSyncReport {
        enabled: state.enabled,
        job_id: super::PRICING_SYNC_CRON_JOB_ID.to_owned(),
        job_exists: state.job_exists,
        status: state.status,
        schedule: state.schedule,
        last_run_at_ms: state.last_run_at_ms,
        next_run_at_ms: state.next_run_at_ms,
        last_run_success: state.last_run_success,
        last_run_summary: state.last_run_summary,
    }
}

/// 速率读数（RPM / TPM）的分母起点。
///
/// 固定周期的分母就是周期起点（该窗口内摊平）。「全部」没有固定窗口：起点取**第一条
/// 事实所在的小时桶**，否则会拿 Unix 纪元当起点（约 2900 万分钟），任何真实用量摊到
/// 那么长的时长里都读成 0，速率一栏形同虚设。窗口内没有任何事实时返回 `now_ms`，
/// 分母退化为 1 分钟且分子为 0，读数仍是 0。
fn rate_window_start_ms(
    period: UsagePeriod,
    period_start_ms: u64,
    hourly: &[(u64, super::model::UsageRollupRow)],
    now_ms: u64,
) -> u64 {
    if period != UsagePeriod::All {
        return period_start_ms;
    }
    hourly
        .iter()
        .map(|(bucket_ms, _)| *bucket_ms)
        .min()
        .unwrap_or(now_ms)
}

fn parse_period(period_str: &str) -> Result<UsagePeriod, String> {
    match period_str.to_ascii_lowercase().as_str() {
        "today" => Ok(UsagePeriod::Today),
        "7d" => Ok(UsagePeriod::SevenDays),
        "30d" => Ok(UsagePeriod::ThirtyDays),
        "all" => Ok(UsagePeriod::All),
        other => Err(format!(
            "未知 period 参数: {other}，白名单为 today, 7d, 30d, all"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage_statistics::model::UsageRollupRow;

    fn hourly_bucket(bucket_ms: u64) -> (u64, UsageRollupRow) {
        (
            bucket_ms,
            UsageRollupRow {
                provider: "p".to_owned(),
                model: "m".to_owned(),
                source: "axon_cli".to_owned(),
                status: "completed".to_owned(),
                request_count: 1,
                provider_call_count: 1,
                total_input_tokens: 0,
                cache_read_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                provider_duration_ms: 0,
                measured_latency_count: 0,
                priced_cost_micros: 0,
                unpriced_request_count: 0,
            },
        )
    }

    #[test]
    fn rate_window_starts_at_the_first_fact_for_the_all_period() {
        let now = 1_792_600_000_000;
        let first = now - 3 * 86_400_000;
        let hourly = vec![hourly_bucket(first), hourly_bucket(now - 3_600_000)];
        assert_eq!(
            rate_window_start_ms(UsagePeriod::All, 0, &hourly, now),
            first,
            "「全部」的分母从第一条事实起算，而不是 Unix 纪元"
        );
    }

    #[test]
    fn rate_window_keeps_the_period_start_for_fixed_windows() {
        let now = 1_792_600_000_000;
        let midnight = now - 12 * 3_600_000;
        let hourly = vec![hourly_bucket(now - 3_600_000)];
        for period in [
            UsagePeriod::Today,
            UsagePeriod::SevenDays,
            UsagePeriod::ThirtyDays,
        ] {
            assert_eq!(
                rate_window_start_ms(period, midnight, &hourly, now),
                midnight,
                "固定周期的分母是该周期的起点"
            );
        }
    }

    #[test]
    fn rate_window_falls_back_to_now_when_the_window_is_empty() {
        let now = 1_792_600_000_000;
        assert_eq!(rate_window_start_ms(UsagePeriod::All, 0, &[], now), now);
    }
}
