//! 会话累计指标的投影更新。
//!
//! `session_metrics` 是投影表：每一行都能由事件流里的 `turn_metrics` 与作业事件重放
//! 得到。因此这里的写入失败不阻断主流程，下一次重放可以修正。

use std::collections::BTreeMap;

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::ports::RuntimeStoreError;
use axon_domain::DaemonSessionId;
use axon_eventlog::{TurnMetrics, TurnOutcome};

/// 直方图桶边界（毫秒）。共 16 桶，最后一桶是「大于等于 60 秒」。
const HISTOGRAM_BOUNDS_MS: [u64; 15] = [
    1, 2, 5, 10, 20, 50, 100, 200, 500, 1_000, 2_000, 5_000, 10_000, 30_000, 60_000,
];
/// 直方图桶数。
const HISTOGRAM_BUCKETS: usize = 16;

/// 把一轮指标累加进会话投影。
pub(super) fn apply_turn_metrics(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    metrics: &TurnMetrics,
    channel: Option<&str>,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let existing = read_row(&transaction, session_id)?;
    let mut row = existing.unwrap_or_default();

    row.turn_count = row.turn_count.saturating_add(1);
    match metrics.outcome {
        TurnOutcome::Succeeded => row.turn_succeeded = row.turn_succeeded.saturating_add(1),
        TurnOutcome::Failed => row.turn_failed = row.turn_failed.saturating_add(1),
        TurnOutcome::Interrupted => {
            row.turn_interrupted = row.turn_interrupted.saturating_add(1);
        }
    }
    row.total_active_ms = row.total_active_ms.saturating_add(metrics.active_ms);
    row.total_provider_ms = row
        .total_provider_ms
        .saturating_add(metrics.provider_duration_ms);
    row.total_tool_ms = row.total_tool_ms.saturating_add(metrics.tool_ms);
    row.max_turn_ms = row.max_turn_ms.max(metrics.active_ms);
    if let Some(ttft) = metrics.ttft_ms {
        row.ttft_sum_ms = row.ttft_sum_ms.saturating_add(ttft);
        row.ttft_samples = row.ttft_samples.saturating_add(1);
        row.ttft_max_ms = row.ttft_max_ms.max(ttft);
        add_bucket(&mut row.ttft_histogram, ttft);
    }
    add_bucket(&mut row.turn_histogram, metrics.active_ms);
    row.total_input_tokens = row.total_input_tokens.saturating_add(metrics.input_tokens);
    row.total_output_tokens = row
        .total_output_tokens
        .saturating_add(metrics.output_tokens);
    row.total_reasoning_tokens = row
        .total_reasoning_tokens
        .saturating_add(metrics.reasoning_tokens);
    row.total_cache_read_tokens = row
        .total_cache_read_tokens
        .saturating_add(metrics.cache_read_tokens);
    row.total_cache_write_tokens = row
        .total_cache_write_tokens
        .saturating_add(metrics.cache_write_tokens);
    row.total_tool_calls = row.total_tool_calls.saturating_add(u64::from(metrics.tool_calls));
    let channel_key = channel.unwrap_or("internal").to_owned();
    let channel_entry = row.channel_mix.entry(channel_key).or_insert(0);
    *channel_entry = channel_entry.saturating_add(1);

    row.first_turn_at_ms.get_or_insert(now_ms);
    row.last_turn_at_ms = Some(now_ms);
    row.updated_at_ms = now_ms;

    write_row(&transaction, session_id, &row)?;
    transaction.commit().map_err(storage)
}

/// 后台任务计数变更：`+1` 表示开始，`-1` 表示结束。
pub(super) fn apply_job_delta(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    started: bool,
    failed: bool,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    transaction
        .execute(
            "INSERT INTO session_metrics(
                 session_id, jobs_started, jobs_completed, jobs_failed, jobs_running, updated_at_ms
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(session_id) DO UPDATE SET
                 jobs_started = session_metrics.jobs_started + excluded.jobs_started,
                 jobs_completed = session_metrics.jobs_completed + excluded.jobs_completed,
                 jobs_failed = session_metrics.jobs_failed + excluded.jobs_failed,
                 jobs_running = MAX(session_metrics.jobs_running + excluded.jobs_running, 0),
                 updated_at_ms = excluded.updated_at_ms",
            params![
                session_id.as_str(),
                i64::from(started),
                i64::from(!started),
                i64::from(!started && failed),
                i64::from(started) - i64::from(!started),
                i64_value(now_ms)?
            ],
        )
        .map_err(storage)?;
    transaction.commit().map_err(storage)
}

/// 读取会话累计指标；不存在时返回 `None`。
pub(crate) fn read_metrics(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Result<Option<SessionMetricsSnapshot>, RuntimeStoreError> {
    let connection = store.connection()?;
    read_snapshot(&connection, session_id)
}

/// 会话累计指标的可读快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionMetricsSnapshot {
    /// 轮次数。
    pub turn_count: u64,
    /// 成功轮次。
    pub turn_succeeded: u64,
    /// 失败轮次。
    pub turn_failed: u64,
    /// 中断轮次。
    pub turn_interrupted: u64,
    /// 累计轮次墙钟耗时。
    pub total_active_ms: u64,
    /// 累计模型往返耗时。
    pub total_provider_ms: u64,
    /// 累计工具耗时。
    pub total_tool_ms: u64,
    /// 单轮最长耗时。
    pub max_turn_ms: u64,
    /// 首字延迟样本和。
    pub ttft_sum_ms: u64,
    /// 首字延迟样本数。
    pub ttft_samples: u64,
    /// 首字延迟最大值。
    pub ttft_max_ms: u64,
    /// 首字延迟直方图（16 桶）。
    pub ttft_histogram: Vec<u64>,
    /// 轮次耗时直方图（16 桶）。
    pub turn_histogram: Vec<u64>,
    /// 渠道分布。
    pub channel_mix: BTreeMap<String, u64>,
    /// 输入 token 累计。
    pub total_input_tokens: u64,
    /// 输出 token 累计。
    pub total_output_tokens: u64,
    /// 推理 token 累计。
    pub total_reasoning_tokens: u64,
    /// 缓存读取 token 累计。
    pub total_cache_read_tokens: u64,
    /// 缓存写入 token 累计。
    pub total_cache_write_tokens: u64,
    /// 工具调用次数累计。
    pub total_tool_calls: u64,
    /// 后台任务启动数。
    pub jobs_started: u64,
    /// 后台任务完成数。
    pub jobs_completed: u64,
    /// 后台任务失败数。
    pub jobs_failed: u64,
    /// 当前在跑的后台任务数。
    pub jobs_running: u64,
    /// 首轮时间。
    pub first_turn_at_ms: Option<u64>,
    /// 末轮时间。
    pub last_turn_at_ms: Option<u64>,
    /// 更新时间。
    pub updated_at_ms: u64,
}

#[derive(Default)]
struct MetricsRow {
    turn_count: u64,
    turn_succeeded: u64,
    turn_failed: u64,
    turn_interrupted: u64,
    total_active_ms: u64,
    total_provider_ms: u64,
    total_tool_ms: u64,
    max_turn_ms: u64,
    ttft_sum_ms: u64,
    ttft_samples: u64,
    ttft_max_ms: u64,
    ttft_histogram: Vec<u64>,
    turn_histogram: Vec<u64>,
    channel_mix: BTreeMap<String, u64>,
    total_input_tokens: u64,
    total_output_tokens: u64,
    total_reasoning_tokens: u64,
    total_cache_read_tokens: u64,
    total_cache_write_tokens: u64,
    total_tool_calls: u64,
    jobs_started: u64,
    jobs_completed: u64,
    jobs_failed: u64,
    jobs_running: u64,
    first_turn_at_ms: Option<u64>,
    last_turn_at_ms: Option<u64>,
    updated_at_ms: u64,
}

fn add_bucket(histogram: &mut Vec<u64>, value_ms: u64) {
    if histogram.len() < HISTOGRAM_BUCKETS {
        histogram.resize(HISTOGRAM_BUCKETS, 0);
    }
    let index = HISTOGRAM_BOUNDS_MS
        .iter()
        .position(|bound| value_ms < *bound)
        .unwrap_or(HISTOGRAM_BUCKETS - 1);
    if let Some(slot) = histogram.get_mut(index) {
        *slot = slot.saturating_add(1);
    }
}

fn read_row(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &DaemonSessionId,
) -> Result<Option<MetricsRow>, RuntimeStoreError> {
    let row = transaction
        .query_row(
            "SELECT turn_count,turn_succeeded,turn_failed,turn_interrupted,
                    total_active_ms,total_provider_ms,total_tool_ms,max_turn_ms,
                    ttft_sum_ms,ttft_samples,ttft_max_ms,ttft_histogram_json,turn_histogram_json,
                    channel_mix_json,total_input_tokens,total_output_tokens,total_reasoning_tokens,
                    total_cache_read_tokens,total_cache_write_tokens,total_tool_calls,
                    jobs_started,jobs_completed,jobs_failed,jobs_running,
                    first_turn_at_ms,last_turn_at_ms,updated_at_ms
             FROM session_metrics WHERE session_id=?1",
            params![session_id.as_str()],
            |row| {
                Ok(MetricsRow {
                    turn_count: u64_value(row.get(0)?),
                    turn_succeeded: u64_value(row.get(1)?),
                    turn_failed: u64_value(row.get(2)?),
                    turn_interrupted: u64_value(row.get(3)?),
                    total_active_ms: u64_value(row.get(4)?),
                    total_provider_ms: u64_value(row.get(5)?),
                    total_tool_ms: u64_value(row.get(6)?),
                    max_turn_ms: u64_value(row.get(7)?),
                    ttft_sum_ms: u64_value(row.get(8)?),
                    ttft_samples: u64_value(row.get(9)?),
                    ttft_max_ms: u64_value(row.get(10)?),
                    ttft_histogram: parse_histogram(row.get::<_, String>(11)?),
                    turn_histogram: parse_histogram(row.get::<_, String>(12)?),
                    channel_mix: parse_channel_mix(row.get::<_, String>(13)?),
                    total_input_tokens: u64_value(row.get(14)?),
                    total_output_tokens: u64_value(row.get(15)?),
                    total_reasoning_tokens: u64_value(row.get(16)?),
                    total_cache_read_tokens: u64_value(row.get(17)?),
                    total_cache_write_tokens: u64_value(row.get(18)?),
                    total_tool_calls: u64_value(row.get(19)?),
                    jobs_started: u64_value(row.get(20)?),
                    jobs_completed: u64_value(row.get(21)?),
                    jobs_failed: u64_value(row.get(22)?),
                    jobs_running: u64_value(row.get(23)?),
                    first_turn_at_ms: row.get::<_, Option<i64>>(24)?.and_then(|v| u64::try_from(v).ok()),
                    last_turn_at_ms: row.get::<_, Option<i64>>(25)?.and_then(|v| u64::try_from(v).ok()),
                    updated_at_ms: u64_value(row.get(26)?),
                })
            },
        )
        .optional()
        .map_err(storage)?;
    Ok(row)
}

fn write_row(
    transaction: &rusqlite::Transaction<'_>,
    session_id: &DaemonSessionId,
    row: &MetricsRow,
) -> Result<(), RuntimeStoreError> {
    transaction
        .execute(
            "INSERT INTO session_metrics(
                 session_id,turn_count,turn_succeeded,turn_failed,turn_interrupted,
                 total_active_ms,total_provider_ms,total_tool_ms,max_turn_ms,
                 ttft_sum_ms,ttft_samples,ttft_max_ms,ttft_histogram_json,turn_histogram_json,
                 channel_mix_json,total_input_tokens,total_output_tokens,total_reasoning_tokens,
                 total_cache_read_tokens,total_cache_write_tokens,total_tool_calls,
                 jobs_started,jobs_completed,jobs_failed,jobs_running,
                 first_turn_at_ms,last_turn_at_ms,updated_at_ms
             ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28)
             ON CONFLICT(session_id) DO UPDATE SET
                 turn_count=excluded.turn_count,
                 turn_succeeded=excluded.turn_succeeded,
                 turn_failed=excluded.turn_failed,
                 turn_interrupted=excluded.turn_interrupted,
                 total_active_ms=excluded.total_active_ms,
                 total_provider_ms=excluded.total_provider_ms,
                 total_tool_ms=excluded.total_tool_ms,
                 max_turn_ms=excluded.max_turn_ms,
                 ttft_sum_ms=excluded.ttft_sum_ms,
                 ttft_samples=excluded.ttft_samples,
                 ttft_max_ms=excluded.ttft_max_ms,
                 ttft_histogram_json=excluded.ttft_histogram_json,
                 turn_histogram_json=excluded.turn_histogram_json,
                 channel_mix_json=excluded.channel_mix_json,
                 total_input_tokens=excluded.total_input_tokens,
                 total_output_tokens=excluded.total_output_tokens,
                 total_reasoning_tokens=excluded.total_reasoning_tokens,
                 total_cache_read_tokens=excluded.total_cache_read_tokens,
                 total_cache_write_tokens=excluded.total_cache_write_tokens,
                 total_tool_calls=excluded.total_tool_calls,
                 jobs_started=excluded.jobs_started,
                 jobs_completed=excluded.jobs_completed,
                 jobs_failed=excluded.jobs_failed,
                 jobs_running=excluded.jobs_running,
                 first_turn_at_ms=COALESCE(session_metrics.first_turn_at_ms, excluded.first_turn_at_ms),
                 last_turn_at_ms=excluded.last_turn_at_ms,
                 updated_at_ms=excluded.updated_at_ms",
            params![
                session_id.as_str(),
                i64_value(row.turn_count)?,
                i64_value(row.turn_succeeded)?,
                i64_value(row.turn_failed)?,
                i64_value(row.turn_interrupted)?,
                i64_value(row.total_active_ms)?,
                i64_value(row.total_provider_ms)?,
                i64_value(row.total_tool_ms)?,
                i64_value(row.max_turn_ms)?,
                i64_value(row.ttft_sum_ms)?,
                i64_value(row.ttft_samples)?,
                i64_value(row.ttft_max_ms)?,
                serde_json::to_string(&row.ttft_histogram).unwrap_or_else(|_| "[]".to_owned()),
                serde_json::to_string(&row.turn_histogram).unwrap_or_else(|_| "[]".to_owned()),
                serde_json::to_string(&row.channel_mix).unwrap_or_else(|_| "{}".to_owned()),
                i64_value(row.total_input_tokens)?,
                i64_value(row.total_output_tokens)?,
                i64_value(row.total_reasoning_tokens)?,
                i64_value(row.total_cache_read_tokens)?,
                i64_value(row.total_cache_write_tokens)?,
                i64_value(row.total_tool_calls)?,
                i64_value(row.jobs_started)?,
                i64_value(row.jobs_completed)?,
                i64_value(row.jobs_failed)?,
                i64_value(row.jobs_running)?,
                row.first_turn_at_ms.map(i64_value).transpose()?,
                row.last_turn_at_ms.map(i64_value).transpose()?,
                i64_value(row.updated_at_ms)?,
            ],
        )
        .map_err(storage)?;
    Ok(())
}

fn read_snapshot(
    connection: &rusqlite::Connection,
    session_id: &DaemonSessionId,
) -> Result<Option<SessionMetricsSnapshot>, RuntimeStoreError> {
    let row = connection
        .query_row(
            "SELECT turn_count,turn_succeeded,turn_failed,turn_interrupted,
                    total_active_ms,total_provider_ms,total_tool_ms,max_turn_ms,
                    ttft_sum_ms,ttft_samples,ttft_max_ms,ttft_histogram_json,turn_histogram_json,
                    channel_mix_json,total_input_tokens,total_output_tokens,total_reasoning_tokens,
                    total_cache_read_tokens,total_cache_write_tokens,total_tool_calls,
                    jobs_started,jobs_completed,jobs_failed,jobs_running,
                    first_turn_at_ms,last_turn_at_ms,updated_at_ms
             FROM session_metrics WHERE session_id=?1",
            params![session_id.as_str()],
            |row| {
                Ok(SessionMetricsSnapshot {
                    turn_count: u64_value(row.get(0)?),
                    turn_succeeded: u64_value(row.get(1)?),
                    turn_failed: u64_value(row.get(2)?),
                    turn_interrupted: u64_value(row.get(3)?),
                    total_active_ms: u64_value(row.get(4)?),
                    total_provider_ms: u64_value(row.get(5)?),
                    total_tool_ms: u64_value(row.get(6)?),
                    max_turn_ms: u64_value(row.get(7)?),
                    ttft_sum_ms: u64_value(row.get(8)?),
                    ttft_samples: u64_value(row.get(9)?),
                    ttft_max_ms: u64_value(row.get(10)?),
                    ttft_histogram: parse_histogram(row.get::<_, String>(11)?),
                    turn_histogram: parse_histogram(row.get::<_, String>(12)?),
                    channel_mix: parse_channel_mix(row.get::<_, String>(13)?),
                    total_input_tokens: u64_value(row.get(14)?),
                    total_output_tokens: u64_value(row.get(15)?),
                    total_reasoning_tokens: u64_value(row.get(16)?),
                    total_cache_read_tokens: u64_value(row.get(17)?),
                    total_cache_write_tokens: u64_value(row.get(18)?),
                    total_tool_calls: u64_value(row.get(19)?),
                    jobs_started: u64_value(row.get(20)?),
                    jobs_completed: u64_value(row.get(21)?),
                    jobs_failed: u64_value(row.get(22)?),
                    jobs_running: u64_value(row.get(23)?),
                    first_turn_at_ms: row.get::<_, Option<i64>>(24)?.and_then(|v| u64::try_from(v).ok()),
                    last_turn_at_ms: row.get::<_, Option<i64>>(25)?.and_then(|v| u64::try_from(v).ok()),
                    updated_at_ms: u64_value(row.get(26)?),
                })
            },
        )
        .optional()
        .map_err(storage)?;
    Ok(row)
}

fn u64_value(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn parse_histogram(raw: String) -> Vec<u64> {
    serde_json::from_str::<Vec<u64>>(&raw).unwrap_or_default()
}

fn parse_channel_mix(raw: String) -> BTreeMap<String, u64> {
    serde_json::from_str::<BTreeMap<String, u64>>(&raw).unwrap_or_default()
}
