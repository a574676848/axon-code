use std::path::Path;

use axon_crypto::sha256_hex;
use axon_session::{JsonlThreadEventStore, ThreadService};
use chrono::{DateTime, Datelike as _, Local, LocalResult, TimeZone as _, Timelike as _};

use super::{
    model::{ThreadUsageRecord, UsageFact, UsageProjectionReport, UsageSource, UsageStatus},
    repository::{UsageRepository, UsageSourceRow},
};
const ACTIVE_FACT_RECONCILIATION_LIMIT: usize = 256;
/// 增量读取单个会话时尾部占用的字节预算。
const CHANGED_THREAD_TAIL_BUDGET_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) fn run_usage_rollup_once(config_path: &Path) -> Result<UsageProjectionReport, String> {
    let axon_root = crate::runtime_paths::runtime_axon_root_path_static(config_path)
        .map_err(ToOwned::to_owned)?;
    let mut repository = UsageRepository::open(&axon_root)?;
    let state = repository.projection_state()?;
    let active_keys = repository.active_fact_keys(ACTIVE_FACT_RECONCILIATION_LIMIT)?;
    let source_rows =
        read_thread_usage_rows(&axon_root, state.source_watermark_rowid, &active_keys)?;
    let projected_records = u64::try_from(source_rows.len()).unwrap_or(u64::MAX);

    let mut source_watermark_rowid = state.source_watermark_rowid;
    let pricing_rules = repository.list_pricing_rules()?;
    let mut facts = Vec::with_capacity(source_rows.len());
    for row in source_rows {
        source_watermark_rowid = source_watermark_rowid.max(row.recorded_at_ms);
        facts.push(fact_from_source(
            row.recorded_at_ms,
            &row.record,
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

/// 读取 thread 事件面里的轮次事实。
///
/// 水位是「上次投影到的记录时间（毫秒）」：取 `>=` 而不是 `>`，同毫秒的多轮宁可重
/// 投影一次（事实按 `(sessionId, assistantPartId)` 幂等覆盖），也不能漏掉一轮。
/// 活动事实的键同时参与对账，中途落盘的同一轮在终态更新后必定重读。
///
/// 增量口径：先用 thread 派生索引判断「哪些会话在上次投影之后写过」，只读这些会话的
/// 头部分片 + 尾部分片；索引缺失（旧库首次运行）才回退整表投影。
fn read_thread_usage_rows(
    axon_root: &Path,
    watermark_ms: u64,
    reconcile_keys: &[(String, String)],
) -> Result<Vec<UsageSourceRow>, String> {
    let store = JsonlThreadEventStore;
    let reconcile = reconcile_keys
        .iter()
        .collect::<std::collections::HashSet<_>>();
    let details = match store
        .read_thread_index(axon_root)
        .map_err(|error| format!("读取 thread 索引失败：{error}"))?
    {
        Some(snapshot) => {
            let changed = snapshot
                .entries
                .into_iter()
                .filter(|entry| {
                    entry.updated_at_ms >= watermark_ms
                        || reconcile
                            .iter()
                            .any(|(session_id, _)| session_id == &entry.thread_id)
                })
                .map(|entry| entry.thread_id)
                .collect::<Vec<_>>();
            if changed.is_empty() {
                return Ok(Vec::new());
            }
            read_changed_threads(axon_root, &changed, watermark_ms)?
        }
        None => ThreadService::new(JsonlThreadEventStore)
            .scan_threads(axon_root)
            .map_err(|error| format!("扫描 thread 事件面失败：{error}"))?,
    };
    let mut rows = Vec::new();
    for detail in &details {
        let thread = &detail.thread;
        for turn in &detail.turns {
            let Some(metadata) = turn_metadata(&turn.token_usage_json) else {
                continue;
            };
            let recorded_at_ms = turn
                .completed_at_ms
                .or(turn.interrupted_at_ms)
                .unwrap_or(turn.started_at_ms);
            let key = (thread.thread_id.clone(), metadata.assistant_part_id.clone());
            if recorded_at_ms < watermark_ms && !reconcile.contains(&key) {
                continue;
            }
            rows.push(UsageSourceRow {
                recorded_at_ms,
                record: ThreadUsageRecord {
                    session_id: thread.thread_id.clone(),
                    is_subagent: thread.parent_thread_id.is_some(),
                    origin_channel: metadata.origin_channel.clone(),
                    provider: thread.settings.provider.clone(),
                    model: thread.settings.model.clone(),
                    assistant_part_id: metadata.assistant_part_id.clone(),
                    usage: metadata.usage,
                    status: match turn.status {
                        axon_session::TurnStatus::Completed => UsageStatus::Completed,
                        axon_session::TurnStatus::Failed => UsageStatus::Failed,
                        axon_session::TurnStatus::Interrupted => UsageStatus::Interrupted,
                        axon_session::TurnStatus::Active => UsageStatus::Active,
                    },
                    provider_call_count: metadata.provider_call_count,
                    provider_duration_ms: metadata.provider_duration_ms,
                    upstream_model: metadata.upstream_model.clone(),
                    reasoning_effort: metadata.reasoning_effort.clone(),
                    request_path: metadata.request_path.clone(),
                    speed_mode: metadata.speed_mode.clone(),
                    ttft_ms: metadata.ttft_ms,
                    generation_speed_tps: metadata.generation_speed_tps,
                },
            });
        }
    }
    Ok(rows)
}

/// 轮次 metadata 的只读解码；缺失时该轮不参与用量投影。
fn turn_metadata(
    token_usage_json: &Option<String>,
) -> Option<super::super::run_commands::run_thread_store::ThreadTurnMetadata> {
    serde_json::from_str(token_usage_json.as_deref()?).ok()
}

/// 增量读取「上次投影之后写过」的会话：只读头部分片（settings）与尾部分片（近期轮次）。
///
/// 尾部覆盖不足且水位早于尾部起点时，这部分轮次可能被漏掉，因此该会话回退整条读。
fn read_changed_threads(
    axon_root: &Path,
    thread_ids: &[String],
    watermark_ms: u64,
) -> Result<Vec<axon_session::ThreadDetail>, String> {
    let store = JsonlThreadEventStore;
    let mut details = Vec::with_capacity(thread_ids.len());
    for thread_id in thread_ids {
        let head = store
            .read_thread_head(axon_root, thread_id)
            .map_err(|error| format!("读取 thread 头部失败：{error}"))?;
        let tail = store
            .read_thread_tail(axon_root, thread_id, CHANGED_THREAD_TAIL_BUDGET_BYTES)
            .map_err(|error| format!("读取 thread 尾部失败：{error}"))?;
        /*
         * 索引里还挂着、分片却已经不在了：跳过这一条并留痕。
         *
         * 一条被外部删掉分片的 thread 不能把整桶用量统计永久打死：聚合是按轮次累加的，
         * 少一条幽灵记录只是少一份历史用量，而 hard fail 会让看板从此停在旧水位不动。
         * 头部与尾部都读不到任何记录，就说明这个 thread 已经没有任何可投影的事实。
         */
        if head.is_empty() && tail.records.is_empty() {
            axon_logger::warning(&format!(
                "[usage-rollup] 跳过索引中已无分片的 thread：{thread_id}"
            ));
            continue;
        }
        let tail_start_ms = tail.records.first().map(|record| record.recorded_at_ms);
        let safe = tail.complete || tail_start_ms.is_some_and(|earliest| watermark_ms >= earliest);
        if !safe {
            match ThreadService::new(JsonlThreadEventStore).read_thread(axon_root, thread_id) {
                Ok(detail) => details.push(detail),
                // 同一类幽灵记录：预算外要整条重读时才发现分片不完整，同样跳过而不是整体失败。
                Err(error) => axon_logger::warning(&format!(
                    "[usage-rollup] 跳过不可读 thread {thread_id}（{error}），本次不投影该条"
                )),
            }
            continue;
        }
        let mut records = head;
        records.extend(tail.records);
        records.sort_by_key(|record| record.sequence);
        records.dedup_by_key(|record| record.sequence);
        details.extend(axon_session::project_threads(&records));
    }
    Ok(details)
}

fn fact_from_source(
    recorded_at_ms: u64,
    source: &ThreadUsageRecord,
    pricing_rules: &[axon_domain::ModelPriceRule],
) -> Result<UsageFact, String> {
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
        session_id: source.session_id.clone(),
        assistant_part_id: source.assistant_part_id.clone(),
        recorded_at_ms,
        hour_bucket_ms,
        day_bucket_ms,
        provider: source.provider.clone(),
        model: source.model.clone(),
        source: UsageSource::from_origin(source.is_subagent, source.origin_channel.as_deref()),
        status: source.status,
        total_input_tokens: usage.total_input_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
        provider_calls: source.provider_call_count,
        provider_duration_ms: source.provider_duration_ms,
        usage_observed,
        cost_micros,
        pricing_rule_id,
        recalculated_at_ms: None,
        source_fingerprint: sha256_hex(
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
                source.session_id,
                source.assistant_part_id,
                source.provider,
                source.model,
                source.status.as_str(),
                usage.total_input_tokens,
                usage.cache_read_input_tokens,
                usage.cache_write_input_tokens,
                usage.output_tokens,
                usage.reasoning_output_tokens,
                source.provider_call_count,
                source.provider_duration_ms
            )
            .as_bytes(),
        ),
        upstream_model: source.upstream_model.clone(),
        reasoning_effort: source.reasoning_effort.clone(),
        request_path: source.request_path.clone(),
        speed_mode: source.speed_mode.clone(),
        ttft_ms: source.ttft_ms,
        generation_speed_tps: source.generation_speed_tps,
    })
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
    fn usage_fact_reads_terminal_status_and_provider_counters_from_metadata() {
        let record = ThreadUsageRecord {
            session_id: "session-real".to_owned(),
            is_subagent: false,
            origin_channel: None,
            provider: "provider-real".to_owned(),
            model: "model-real".to_owned(),
            assistant_part_id: "assistant-real".to_owned(),
            usage: None,
            status: UsageStatus::Completed,
            provider_call_count: 1,
            provider_duration_ms: 123,
            upstream_model: None,
            reasoning_effort: None,
            request_path: None,
            speed_mode: None,
            ttft_ms: None,
            generation_speed_tps: None,
        };
        let fact = fact_from_source(axon_time::unix_now_ms(), &record, &[])
            .expect("缺用量的轮次不应阻断 usage 投影");

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

        let record = ThreadUsageRecord {
            session_id: "sess-price".to_owned(),
            is_subagent: false,
            origin_channel: None,
            provider: "openai".to_owned(),
            model: "gpt-4o".to_owned(),
            assistant_part_id: "asst-price".to_owned(),
            usage: Some(axon_agent_runtime::TokenUsageTelemetry {
                input_tokens: 800,
                cache_read_input_tokens: 200,
                cache_write_input_tokens: 0,
                output_tokens: 100,
                reasoning_output_tokens: 0,
                current_context_tokens: 1100,
                total_input_tokens: 1000,
                ..axon_agent_runtime::TokenUsageTelemetry::default()
            }),
            status: UsageStatus::Completed,
            provider_call_count: 1,
            provider_duration_ms: 250,
            upstream_model: None,
            reasoning_effort: None,
            request_path: None,
            speed_mode: None,
            ttft_ms: None,
            generation_speed_tps: None,
        };

        let fact =
            fact_from_source(axon_time::unix_now_ms(), &record, &[rule]).expect("应成功投影并计费");
        // cost_in = (800 * 2_500_000 + 500_000) / 1_000_000 = 2000
        // cost_out = (100 * 10_000_000 + 500_000) / 1_000_000 = 1000
        // cost_read = (200 * 1_250_000 + 500_000) / 1_000_000 = 250
        // total = 3250
        assert_eq!(fact.cost_micros, Some(3250));
        assert_eq!(fact.pricing_rule_id.as_deref(), Some("rule-test-1"));
        assert!(fact.recalculated_at_ms.is_none());
    }
}
