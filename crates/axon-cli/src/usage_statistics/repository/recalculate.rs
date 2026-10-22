//! 存储层重算批处理实现。

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use axon_domain::{ModelPriceRule, calculate_cost_micros, match_rule};
use rusqlite::{Connection, TransactionBehavior, params};

use super::{db_error, from_i64, rebuild_day_bucket, rebuild_hour_bucket, to_i64};

/// 单批重算限制（与清理限额 256 对齐）。
pub(crate) const RECALCULATE_BATCH_LIMIT: usize = 256;

/// 重算分页游标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecalculateCursor {
    pub(crate) recorded_at_ms: u64,
    pub(crate) session_id: String,
    pub(crate) assistant_part_id: String,
}

/// 单批执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BatchRecalculateOutcome {
    pub(crate) scanned_count: u64,
    pub(crate) updated_count: u64,
    pub(crate) skipped_count: u64,
    pub(crate) next_cursor: Option<RecalculateCursor>,
    pub(crate) duration: Duration,
}

struct FactRecalculateRow {
    session_id: String,
    assistant_part_id: String,
    recorded_at_ms: u64,
    hour_bucket_ms: u64,
    day_bucket_ms: u64,
    model: String,
    total_input_tokens: u64,
    cache_read_input_tokens: u64,
    cache_write_input_tokens: u64,
    output_tokens: u64,
    cost_micros: Option<u64>,
    pricing_rule_id: Option<String>,
}

/// 在单个 Immediate 事务中执行一批记录的重算。
pub(super) fn recalculate_batch(
    connection: &mut Connection,
    rules: &[ModelPriceRule],
    cursor: Option<&RecalculateCursor>,
    now_ms: u64,
) -> Result<BatchRecalculateOutcome, String> {
    let start_time = Instant::now();
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(db_error("启动重算批次 Immediate 事务"))?;

    let rows = fetch_batch_rows(&transaction, cursor)?;
    let scanned_count = rows.len() as u64;
    let next_cursor = rows.last().map(|last| RecalculateCursor {
        recorded_at_ms: last.recorded_at_ms,
        session_id: last.session_id.clone(),
        assistant_part_id: last.assistant_part_id.clone(),
    });

    let mut updated_count = 0_u64;
    let mut skipped_count = 0_u64;
    let mut touched_hours = BTreeSet::new();
    let mut touched_days = BTreeSet::new();

    for row in &rows {
        let matched = match_rule(rules, &row.model);
        let (new_cost, new_rule_id) = if let Some(rule) = matched {
            let cost = calculate_cost_micros(
                rule,
                row.total_input_tokens,
                row.cache_read_input_tokens,
                row.cache_write_input_tokens,
                row.output_tokens,
            );
            (Some(cost), Some(rule.id.as_str()))
        } else {
            (None, None)
        };

        let is_idempotent = match (row.pricing_rule_id.as_deref(), new_rule_id) {
            (Some(old_id), Some(new_id)) => old_id == new_id && row.cost_micros == new_cost,
            (None, None) => row.cost_micros.is_none(),
            _ => false,
        };

        if is_idempotent {
            skipped_count += 1;
        } else {
            transaction
                .execute(
                    "UPDATE usage_facts
                     SET cost_micros = ?1,
                         pricing_rule_id = ?2,
                         recalculated_at_ms = ?3
                     WHERE session_id = ?4 AND assistant_part_id = ?5",
                    params![
                        new_cost.map(to_i64).transpose()?,
                        new_rule_id,
                        to_i64(now_ms)?,
                        row.session_id,
                        row.assistant_part_id,
                    ],
                )
                .map_err(db_error("更新 usage fact 计费列"))?;

            touched_hours.insert(row.hour_bucket_ms);
            touched_days.insert(row.day_bucket_ms);
            updated_count += 1;
        }
    }

    // 仅刷新发生变更的 rollup 桶
    for hour in touched_hours {
        rebuild_hour_bucket(&transaction, hour)?;
    }
    for day in touched_days {
        rebuild_day_bucket(&transaction, day)?;
    }

    transaction.commit().map_err(db_error("提交重算批次事务"))?;

    let duration = start_time.elapsed();
    Ok(BatchRecalculateOutcome {
        scanned_count,
        updated_count,
        skipped_count,
        next_cursor,
        duration,
    })
}

fn fetch_batch_rows(
    connection: &Connection,
    cursor: Option<&RecalculateCursor>,
) -> Result<Vec<FactRecalculateRow>, String> {
    let limit = RECALCULATE_BATCH_LIMIT;
    let mut rows = Vec::with_capacity(limit);

    if let Some(c) = cursor {
        let mut stmt = connection
            .prepare(
                "SELECT session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                        model, total_input_tokens, cache_read_input_tokens, cache_write_input_tokens,
                        output_tokens, cost_micros, pricing_rule_id
                 FROM usage_facts
                 WHERE (recorded_at_ms > ?1)
                    OR (recorded_at_ms = ?1 AND session_id > ?2)
                    OR (recorded_at_ms = ?1 AND session_id = ?2 AND assistant_part_id > ?3)
                 ORDER BY recorded_at_ms ASC, session_id ASC, assistant_part_id ASC
                 LIMIT ?4",
            )
            .map_err(db_error("准备重算游标查询"))?;

        let mapped = stmt
            .query_map(
                params![
                    to_i64(c.recorded_at_ms)?,
                    c.session_id,
                    c.assistant_part_id,
                    limit as i64
                ],
                map_fact_recalculate_row,
            )
            .map_err(db_error("执行重算游标查询"))?;

        for r in mapped {
            rows.push(r.map_err(db_error("读取重算事实行"))?);
        }
    } else {
        let mut stmt = connection
            .prepare(
                "SELECT session_id, assistant_part_id, recorded_at_ms, hour_bucket_ms, day_bucket_ms,
                        model, total_input_tokens, cache_read_input_tokens, cache_write_input_tokens,
                        output_tokens, cost_micros, pricing_rule_id
                 FROM usage_facts
                 ORDER BY recorded_at_ms ASC, session_id ASC, assistant_part_id ASC
                 LIMIT ?1",
            )
            .map_err(db_error("准备重算初始查询"))?;

        let mapped = stmt
            .query_map([limit as i64], map_fact_recalculate_row)
            .map_err(db_error("执行重算初始查询"))?;

        for r in mapped {
            rows.push(r.map_err(db_error("读取重算事实行"))?);
        }
    }

    Ok(rows)
}

fn map_fact_recalculate_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FactRecalculateRow> {
    let cost_raw: Option<i64> = row.get(10)?;
    let cost_micros = cost_raw.map(from_i64).transpose()?;
    Ok(FactRecalculateRow {
        session_id: row.get(0)?,
        assistant_part_id: row.get(1)?,
        recorded_at_ms: from_i64(row.get(2)?)?,
        hour_bucket_ms: from_i64(row.get(3)?)?,
        day_bucket_ms: from_i64(row.get(4)?)?,
        model: row.get(5)?,
        total_input_tokens: from_i64(row.get(6)?)?,
        cache_read_input_tokens: from_i64(row.get(7)?)?,
        cache_write_input_tokens: from_i64(row.get(8)?)?,
        output_tokens: from_i64(row.get(9)?)?,
        cost_micros,
        pricing_rule_id: row.get(11)?,
    })
}
