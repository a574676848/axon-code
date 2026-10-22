use std::path::Path;

use axon_crypto::sha256_hex;
use chrono::{DateTime, Datelike as _, Local, LocalResult, TimeZone as _, Timelike as _};

use super::{
    model::{
        RolloutUsageEvent, RolloutUsageEventKind, RolloutUsageRecord, UsageFact,
        UsageProjectionReport, UsageSource, UsageStatus,
    },
    repository::{UsageRepository, read_rollout_usage_rows},
};
const ACTIVE_FACT_RECONCILIATION_LIMIT: usize = 256;

pub(crate) fn run_usage_rollup_once(config_path: &Path) -> Result<UsageProjectionReport, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let rollout_path =
        crate::run_commands::resolved_rollout_db_path(config_path).map_err(ToOwned::to_owned)?;
    let mut repository = UsageRepository::open(&axon_root)?;
    let state = repository.projection_state()?;
    let active_keys = repository.active_fact_keys(ACTIVE_FACT_RECONCILIATION_LIMIT)?;
    let source_rows =
        read_rollout_usage_rows(&rollout_path, state.source_watermark_rowid, &active_keys)?;
    let projected_records = u64::try_from(source_rows.len()).unwrap_or(u64::MAX);
    let mut source_watermark_rowid = state.source_watermark_rowid;
    let pricing_rules = repository.list_pricing_rules()?;
    let mut facts = Vec::with_capacity(source_rows.len());
    for row in source_rows {
        source_watermark_rowid = source_watermark_rowid.max(row.source_rowid);
        facts.push(fact_from_source(
            row.recorded_at_ms,
            &row.record_json,
            &pricing_rules,
        )?);
    }
    let completed_at_ms = axon_time::unix_now_ms();
    let changed_facts =
        repository.project_facts(&facts, source_watermark_rowid, completed_at_ms)?;
    Ok(UsageProjectionReport {
        projected_records,
        changed_facts,
        source_watermark_rowid,
        completed_at_ms,
    })
}

fn fact_from_source(
    recorded_at_ms: u64,
    record_json: &str,
    pricing_rules: &[axon_domain::ModelPriceRule],
) -> Result<UsageFact, String> {
    let source: RolloutUsageRecord = serde_json::from_str(record_json)
        .map_err(|error| format!("rollout usage 记录反序列化失败：{error}"))?;
    let (hour_bucket_ms, day_bucket_ms) = local_buckets(recorded_at_ms)?;
    let usage_observed = source.usage.is_some();
    let usage = source.usage.unwrap_or_default();

    let (cost_micros, pricing_rule_id) =
        if let Some(rule) = axon_domain::match_rule(pricing_rules, &source.model) {
            let cost = axon_domain::calculate_cost_micros(
                rule,
                usage.total_input_tokens,
                usage.cache_read_input_tokens,
                usage.cache_write_input_tokens,
                usage.output_tokens,
            );
            (Some(cost), Some(rule.id.clone()))
        } else {
            (None, None)
        };

    Ok(UsageFact {
        session_id: source.session_id,
        assistant_part_id: source.assistant_part_id,
        recorded_at_ms,
        hour_bucket_ms,
        day_bucket_ms,
        provider: source.provider,
        model: source.model,
        source: if source.parent_session_id.is_some() {
            UsageSource::Subagent
        } else {
            UsageSource::AxonCli
        },
        status: terminal_status(&source.turn_events),
        total_input_tokens: usage.total_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        provider_calls: provider_call_count(&source.turn_events),
        provider_duration_ms: provider_duration_ms(&source.turn_events),
        usage_observed,
        cost_micros,
        pricing_rule_id,
        recalculated_at_ms: None,
        source_fingerprint: sha256_hex(record_json.as_bytes()),
        upstream_model: source.upstream_model,
        reasoning_effort: source.reasoning_effort,
        request_path: source.request_path,
        speed_mode: source.speed_mode,
        ttft_ms: source.ttft_ms,
        generation_speed_tps: source.generation_speed_tps,
    })
}

fn terminal_status(events: &[RolloutUsageEvent]) -> UsageStatus {
    events
        .iter()
        .rev()
        .find_map(|event| match event.kind {
            RolloutUsageEventKind::Completed => Some(UsageStatus::Completed),
            RolloutUsageEventKind::Failed => Some(UsageStatus::Failed),
            RolloutUsageEventKind::Interrupted => Some(UsageStatus::Interrupted),
            RolloutUsageEventKind::ProviderCompleted | RolloutUsageEventKind::Other => None,
        })
        .unwrap_or(UsageStatus::Active)
}

fn provider_call_count(events: &[RolloutUsageEvent]) -> u64 {
    u64::try_from(
        events
            .iter()
            .filter(|event| event.kind == RolloutUsageEventKind::ProviderCompleted)
            .count(),
    )
    .unwrap_or(u64::MAX)
}

fn provider_duration_ms(events: &[RolloutUsageEvent]) -> u64 {
    events
        .iter()
        .filter(|event| event.kind == RolloutUsageEventKind::ProviderCompleted)
        .filter_map(|event| event.duration_ms)
        .fold(0_u64, u64::saturating_add)
}

pub(super) fn period_start_ms(
    period: super::model::UsagePeriod,
    now_ms: u64,
) -> Result<u64, String> {
    if period == super::model::UsagePeriod::All {
        return Ok(0);
    }
    let now = local_datetime(now_ms)?;
    let days_back = match period {
        super::model::UsagePeriod::Today => 0,
        super::model::UsagePeriod::SevenDays => 6,
        super::model::UsagePeriod::ThirtyDays => 29,
        super::model::UsagePeriod::All => unreachable!(),
    };
    let date = now
        .date_naive()
        .checked_sub_days(chrono::Days::new(days_back))
        .ok_or_else(|| "usage 查询周期超出本地日期范围".to_owned())?;
    local_timestamp_ms(date.year(), date.month(), date.day(), 0)
}

fn local_buckets(recorded_at_ms: u64) -> Result<(u64, u64), String> {
    let local = local_datetime(recorded_at_ms)?;
    Ok((
        local_timestamp_ms(local.year(), local.month(), local.day(), local.hour())?,
        local_timestamp_ms(local.year(), local.month(), local.day(), 0)?,
    ))
}

fn local_datetime(timestamp_ms: u64) -> Result<DateTime<Local>, String> {
    let timestamp_ms =
        i64::try_from(timestamp_ms).map_err(|_| "usage 时间戳超出本地时间范围".to_owned())?;
    Local
        .timestamp_millis_opt(timestamp_ms)
        .single()
        .ok_or_else(|| "usage 时间戳无法转换为本地时间".to_owned())
}

fn local_timestamp_ms(year: i32, month: u32, day: u32, hour: u32) -> Result<u64, String> {
    let resolved = match Local.with_ymd_and_hms(year, month, day, hour, 0, 0) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, second) => first.min(second),
        LocalResult::None => return Err("usage 本地时间 bucket 不存在".to_owned()),
    };
    u64::try_from(resolved.timestamp_millis())
        .map_err(|_| "usage 本地时间 bucket 早于 Unix epoch".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_start_uses_local_calendar_days() {
        let now = Local
            .with_ymd_and_hms(2026, 7, 27, 12, 30, 0)
            .single()
            .expect("本地测试时间应有效");
        let now_ms = u64::try_from(now.timestamp_millis()).expect("测试时间应有效");
        let start = period_start_ms(super::super::model::UsagePeriod::SevenDays, now_ms)
            .expect("周期应可计算");
        let start = local_datetime(start).expect("开始时间应有效");
        assert_eq!((start.year(), start.month(), start.day()), (2026, 7, 21));
        assert_eq!((start.hour(), start.minute()), (0, 0));
    }

    #[test]
    fn usage_fact_ignores_unknown_rollout_event_kinds() {
        let record = serde_json::json!({
            "sessionId": "session-real-rollout",
            "provider": "provider-real-rollout",
            "model": "model-real-rollout",
            "assistantPartId": "assistant-real-rollout",
            "turnEvents": [
                { "kind": "DiagnosticsErrorDetected", "durationMs": 999 },
                { "kind": "ProviderCompleted", "durationMs": 123 },
                { "kind": "Completed" }
            ]
        });
        let fact = fact_from_source(axon_time::unix_now_ms(), &record.to_string(), &[])
            .expect("未知 rollout 事件不应阻断 usage 投影");

        assert_eq!(fact.status, UsageStatus::Completed);
        assert_eq!(fact.provider_calls, 1);
        assert_eq!(fact.provider_duration_ms, 123);
        assert!(!fact.usage_observed);
        assert!(fact.cost_micros.is_none());
        assert!(fact.pricing_rule_id.is_none());
    }

    #[test]
    fn fact_from_source_applies_matched_pricing_rule() {
        let rule = axon_domain::ModelPriceRule {
            id: "rule-test-1".to_string(),
            model_pattern: "gpt-4o*".to_string(),
            pricing_style: axon_domain::PricingStyle::OpenAi,
            input_price_per_m_micros: 2_500_000,
            output_price_per_m_micros: 10_000_000,
            cache_read_price_per_m_micros: 1_250_000,
            cache_write_price_per_m_micros: 0,
            multiplier_basis_points: 10_000,
            priority: 1,
            created_at_ms: 1000,
            updated_at_ms: 1000,
        };

        let record = serde_json::json!({
            "sessionId": "sess-price",
            "provider": "openai",
            "model": "gpt-4o",
            "assistantPartId": "asst-price",
            "usage": {
                "inputTokens": 800,
                "cacheReadInputTokens": 200,
                "cacheWriteInputTokens": 0,
                "outputTokens": 100,
                "reasoningOutputTokens": 0,
                "totalInputTokens": 1000
            },
            "turnEvents": [
                { "kind": "Completed" }
            ]
        });

        let fact = fact_from_source(axon_time::unix_now_ms(), &record.to_string(), &[rule])
            .expect("应成功投影并计费");

        // uncached = 1000 - 200 = 800
        // cost_in = (800 * 2_500_000 + 500_000) / 1_000_000 = 2000
        // cost_out = (100 * 10_000_000 + 500_000) / 1_000_000 = 1000
        // cost_read = (200 * 1_250_000 + 500_000) / 1_000_000 = 250
        // total = 3250
        assert_eq!(fact.cost_micros, Some(3250));
        assert_eq!(fact.pricing_rule_id.as_deref(), Some("rule-test-1"));
        assert!(fact.recalculated_at_ms.is_none());
    }
}
