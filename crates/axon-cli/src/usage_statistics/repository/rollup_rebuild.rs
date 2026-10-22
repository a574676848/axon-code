//! 时序聚合表 (hour / day / minute) 按桶物理重建操作。
//!
//! 供事实落库批量触发或运维一致性修复时使用。

use rusqlite::Transaction;

use super::{db_error, minute_rollup, to_i64};

pub(crate) fn rebuild_hour_bucket(
    transaction: &Transaction<'_>,
    bucket: u64,
) -> Result<(), String> {
    rebuild_bucket(transaction, "usage_rollup_hour", "hour_bucket_ms", bucket)
}

pub(crate) fn rebuild_day_bucket(transaction: &Transaction<'_>, bucket: u64) -> Result<(), String> {
    rebuild_bucket(transaction, "usage_rollup_day", "day_bucket_ms", bucket)
}

pub(crate) fn rebuild_minute_bucket(
    transaction: &Transaction<'_>,
    bucket: u64,
) -> Result<(), String> {
    minute_rollup::rebuild_minute_bucket(transaction, bucket)
}

fn rebuild_bucket(
    transaction: &Transaction<'_>,
    table: &'static str,
    bucket_column: &'static str,
    bucket: u64,
) -> Result<(), String> {
    let bucket = to_i64(bucket)?;
    transaction
        .execute(
            &format!("DELETE FROM {table} WHERE bucket_start_ms = ?1"),
            [bucket],
        )
        .map_err(db_error("清理 usage rollup bucket"))?;
    transaction
        .execute(
            &format!(
                "INSERT INTO {table} (
                    bucket_start_ms, provider, model, source, status,
                    request_count, provider_call_count, total_input_tokens,
                    cache_read_input_tokens, cache_write_input_tokens, output_tokens,
                    reasoning_output_tokens, provider_duration_ms, measured_latency_count,
                    priced_cost_micros, unpriced_request_count
                 )
                 SELECT {bucket_column}, provider, model, source, status,
                    COUNT(*), SUM(provider_calls), SUM(total_input_tokens),
                    SUM(cache_read_input_tokens), SUM(cache_write_input_tokens),
                    SUM(output_tokens), SUM(reasoning_output_tokens),
                    SUM(provider_duration_ms),
                    SUM(CASE WHEN provider_duration_ms > 0 THEN 1 ELSE 0 END),
                    SUM(COALESCE(cost_micros, 0)),
                    SUM(CASE WHEN cost_micros IS NULL THEN 1 ELSE 0 END)
                 FROM usage_facts
                 WHERE {bucket_column} = ?1
                 GROUP BY {bucket_column}, provider, model, source, status"
            ),
            [bucket],
        )
        .map_err(db_error("重建 usage rollup bucket"))?;
    Ok(())
}
