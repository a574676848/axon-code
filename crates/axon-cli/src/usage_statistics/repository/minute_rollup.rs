//! 分钟级时序聚合表 (`usage_rollup_minute`) 仓储操作。
//!
//! 支持：
//! 1. 与事实落库同事务的增量 upsert（ON CONFLICT DO UPDATE）；
//! 2. 基于 `usage_facts` 的按桶重建（供重算/修复，与增量累加结果逐列等价）；
//! 3. 分钟聚合数据检索（供诊断与可观测性看板读取）。

use rusqlite::{Connection, Transaction, params};
use serde::{Deserialize, Serialize};

use super::{db_error, to_i64};
use crate::usage_statistics::model::UsageFact;

/// 分钟桶时长（60 秒 = 60,000 毫秒）。
pub const MINUTE_BUCKET_DURATION_MS: u64 = 60_000;

/// 计算毫秒时间戳对应的分钟桶对齐时间。
#[must_use]
pub fn minute_bucket_ms(timestamp_ms: u64) -> u64 {
    (timestamp_ms / MINUTE_BUCKET_DURATION_MS) * MINUTE_BUCKET_DURATION_MS
}

/// 分钟级聚合行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinuteRollupRow {
    pub bucket_start_ms: u64,
    pub provider: String,
    pub model: String,
    pub source: String,
    pub status: String,
    pub request_count: u64,
    pub total_input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_write_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub provider_duration_ms: u64,
    pub ttft_duration_ms: u64,
    pub measured_ttft_count: u64,
    pub priced_cost_micros: u64,
    pub unpriced_request_count: u64,
}

/// 增量写入单条事实到 `usage_rollup_minute`。
pub(super) fn upsert_minute_rollup(
    transaction: &Transaction<'_>,
    fact: &UsageFact,
) -> Result<u64, String> {
    let bucket_start = minute_bucket_ms(fact.recorded_at_ms);
    let ttft_duration = fact.ttft_ms.unwrap_or(0);
    let measured_ttft: i64 = if fact.ttft_ms.is_some() { 1 } else { 0 };
    let priced_cost = fact.cost_micros.unwrap_or(0);
    let unpriced: i64 = if fact.cost_micros.is_none() { 1 } else { 0 };

    let changed = transaction
        .execute(
            "INSERT INTO usage_rollup_minute (
                bucket_start_ms, provider, model, source, status,
                request_count, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                priced_cost_micros, unpriced_request_count
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5,
                ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
             )
             ON CONFLICT(bucket_start_ms, provider, model, source, status) DO UPDATE SET
                request_count = usage_rollup_minute.request_count + excluded.request_count,
                total_input_tokens = usage_rollup_minute.total_input_tokens + excluded.total_input_tokens,
                cache_read_input_tokens = usage_rollup_minute.cache_read_input_tokens + excluded.cache_read_input_tokens,
                cache_write_input_tokens = usage_rollup_minute.cache_write_input_tokens + excluded.cache_write_input_tokens,
                output_tokens = usage_rollup_minute.output_tokens + excluded.output_tokens,
                reasoning_output_tokens = usage_rollup_minute.reasoning_output_tokens + excluded.reasoning_output_tokens,
                provider_duration_ms = usage_rollup_minute.provider_duration_ms + excluded.provider_duration_ms,
                ttft_duration_ms = usage_rollup_minute.ttft_duration_ms + excluded.ttft_duration_ms,
                measured_ttft_count = usage_rollup_minute.measured_ttft_count + excluded.measured_ttft_count,
                priced_cost_micros = usage_rollup_minute.priced_cost_micros + excluded.priced_cost_micros,
                unpriced_request_count = usage_rollup_minute.unpriced_request_count + excluded.unpriced_request_count",
            params![
                to_i64(bucket_start)?,
                fact.provider,
                fact.model,
                fact.source.as_str(),
                fact.status.as_str(),
                1_i64,
                to_i64(fact.total_input_tokens)?,
                to_i64(fact.cache_read_input_tokens)?,
                to_i64(fact.cache_write_input_tokens)?,
                to_i64(fact.output_tokens)?,
                to_i64(fact.reasoning_output_tokens)?,
                to_i64(fact.provider_duration_ms)?,
                to_i64(ttft_duration)?,
                measured_ttft,
                to_i64(priced_cost)?,
                unpriced,
            ],
        )
        .map_err(db_error("增量写入 usage_rollup_minute"))?;

    u64::try_from(changed).map_err(|_| "usage_rollup_minute 变更数超出范围".to_owned())
}

/// 按桶从 `usage_facts` 原始事实重新构建 `usage_rollup_minute` 某分钟桶。
///
/// 保证与增量累加写入路径的结果逐列一致。
pub(super) fn rebuild_minute_bucket(
    transaction: &Transaction<'_>,
    bucket: u64,
) -> Result<(), String> {
    let bucket_start = minute_bucket_ms(bucket);
    let bucket_end = bucket_start.saturating_add(MINUTE_BUCKET_DURATION_MS);
    let start_i64 = to_i64(bucket_start)?;
    let end_i64 = to_i64(bucket_end)?;

    transaction
        .execute(
            "DELETE FROM usage_rollup_minute WHERE bucket_start_ms = ?1",
            [start_i64],
        )
        .map_err(db_error("清理 usage_rollup_minute 桶"))?;

    transaction
        .execute(
            "INSERT INTO usage_rollup_minute (
                bucket_start_ms, provider, model, source, status,
                request_count, total_input_tokens, cache_read_input_tokens,
                cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                priced_cost_micros, unpriced_request_count
             )
             SELECT
                ?1, provider, model, source, status,
                COUNT(*),
                SUM(total_input_tokens),
                SUM(cache_read_input_tokens),
                SUM(cache_write_input_tokens),
                SUM(output_tokens),
                SUM(reasoning_output_tokens),
                SUM(provider_duration_ms),
                SUM(COALESCE(ttft_ms, 0)),
                SUM(CASE WHEN ttft_ms IS NOT NULL THEN 1 ELSE 0 END),
                SUM(COALESCE(cost_micros, 0)),
                SUM(CASE WHEN cost_micros IS NULL THEN 1 ELSE 0 END)
             FROM usage_facts
             WHERE recorded_at_ms >= ?1 AND recorded_at_ms < ?2
             GROUP BY provider, model, source, status",
            [start_i64, end_i64],
        )
        .map_err(db_error("重建 usage_rollup_minute 桶"))?;

    Ok(())
}

/// 查询指定时间之后的分钟聚合数据。
pub fn query_minute_rollups(
    connection: &Connection,
    since_ms: u64,
    limit: usize,
) -> Result<Vec<MinuteRollupRow>, String> {
    let since_i64 = to_i64(since_ms)?;
    let limit_i64 = i64::try_from(limit).unwrap_or(100);

    let mut stmt = connection
        .prepare(
            "SELECT bucket_start_ms, provider, model, source, status,
                    request_count, total_input_tokens, cache_read_input_tokens,
                    cache_write_input_tokens, output_tokens, reasoning_output_tokens,
                    provider_duration_ms, ttft_duration_ms, measured_ttft_count,
                    priced_cost_micros, unpriced_request_count
             FROM usage_rollup_minute
             WHERE bucket_start_ms >= ?1
             ORDER BY bucket_start_ms DESC
             LIMIT ?2",
        )
        .map_err(db_error("准备查询 usage_rollup_minute"))?;

    let rows = stmt
        .query_map(params![since_i64, limit_i64], |row| {
            Ok(MinuteRollupRow {
                bucket_start_ms: row.get::<_, i64>(0)? as u64,
                provider: row.get(1)?,
                model: row.get(2)?,
                source: row.get(3)?,
                status: row.get(4)?,
                request_count: row.get::<_, i64>(5)? as u64,
                total_input_tokens: row.get::<_, i64>(6)? as u64,
                cache_read_input_tokens: row.get::<_, i64>(7)? as u64,
                cache_write_input_tokens: row.get::<_, i64>(8)? as u64,
                output_tokens: row.get::<_, i64>(9)? as u64,
                reasoning_output_tokens: row.get::<_, i64>(10)? as u64,
                provider_duration_ms: row.get::<_, i64>(11)? as u64,
                ttft_duration_ms: row.get::<_, i64>(12)? as u64,
                measured_ttft_count: row.get::<_, i64>(13)? as u64,
                priced_cost_micros: row.get::<_, i64>(14)? as u64,
                unpriced_request_count: row.get::<_, i64>(15)? as u64,
            })
        })
        .map_err(db_error("查询 usage_rollup_minute"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(db_error("解析 usage_rollup_minute 数据"))?;

    Ok(rows)
}
