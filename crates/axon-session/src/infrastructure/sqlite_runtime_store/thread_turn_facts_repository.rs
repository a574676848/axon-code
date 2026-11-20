//! 轮次事实侧车表：把「车道当前模型 / 思考强度 / 最近一次记忆反思 / 轮序」从大记录里剥离。
//!
//! 为什么需要它：这些字段只存在于 thread 轮次的 metadata 里，而一条轮次记录带着完整
//! 历史 items，实测 676 KB 到数 MB。打开会话只想读这几个标量，原先要读 4 MiB 尾窗；
//! 单条记录超过窗口时还得退回整条 thread（实测单会话 80 MiB）。侧车表把这次读降成一次
//! 主键查询，与历史长度无关。
//!
//! 表是投影：写入失败不阻断主流程，读侧在缺行时仍可回落尾窗读（见 `latest_turn_facts`）。

use rusqlite::{OptionalExtension, Row, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::ports::RuntimeStoreError;

/// 侧车事实的列顺序：两个身份、收口时刻、轮序与三个标量。
const FACT_COLUMNS: &str = "thread_id, assistant_part_id, recorded_at_ms, turn_sequence, model_name, reasoning_effort, reflection_notice";

/// 一轮的标量事实。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadTurnFacts {
    /// thread 标识；与运行会话一一对应。
    pub thread_id: String,
    /// 该轮的 assistant part 标识：轮次在 thread 里的稳定身份。
    pub assistant_part_id: String,
    /// 收口时刻（Unix 毫秒），最新一轮按它取。
    pub recorded_at_ms: u64,
    /// 该轮在 thread 里的轮序；写入方还没结算时为空。
    ///
    /// 轮序是 `trace:<session>:<sequence>` 与按序号定位的寻址键：读侧拿它是为了不再为了
    /// 一个标量去读尾窗（记录本身可能超过窗口，然后退化成整条 thread 读）。
    pub sequence: Option<u64>,
    /// 用户当时选择的模型别名。
    pub model_name: Option<String>,
    /// 该轮实际请求的思考强度。
    pub reasoning_effort: Option<String>,
    /// 轮后治理写入的记忆反思提示。
    pub reflection_notice: Option<String>,
}

/// 按 [`FACT_COLUMNS`] 的列顺序解出一行事实。
fn decode_row(row: &Row<'_>) -> rusqlite::Result<ThreadTurnFacts> {
    Ok(ThreadTurnFacts {
        thread_id: row.get(0)?,
        assistant_part_id: row.get(1)?,
        recorded_at_ms: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
        sequence: row
            .get::<_, Option<i64>>(3)?
            .and_then(|value| u64::try_from(value).ok()),
        model_name: row.get(4)?,
        reasoning_effort: row.get(5)?,
        reflection_notice: row.get(6)?,
    })
}

/// 写入或更新一轮的标量事实。
pub(super) fn upsert_turn_facts(
    store: &SqliteRuntimeStore,
    facts: &ThreadTurnFacts,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let connection = store.connection()?;
    connection
        .execute(
            "INSERT INTO thread_turn_facts (
                 thread_id, assistant_part_id, recorded_at_ms, turn_sequence,
                 model_name, reasoning_effort, reflection_notice, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(thread_id, assistant_part_id) DO UPDATE SET
                 recorded_at_ms = excluded.recorded_at_ms,
                 turn_sequence = COALESCE(excluded.turn_sequence, thread_turn_facts.turn_sequence),
                 model_name = COALESCE(excluded.model_name, thread_turn_facts.model_name),
                 reasoning_effort = COALESCE(excluded.reasoning_effort, thread_turn_facts.reasoning_effort),
                 reflection_notice = COALESCE(excluded.reflection_notice, thread_turn_facts.reflection_notice),
                 updated_at_ms = excluded.updated_at_ms",
            params![
                facts.thread_id,
                facts.assistant_part_id,
                i64_value(facts.recorded_at_ms)?,
                facts.sequence.map(i64_value).transpose()?,
                facts.model_name,
                facts.reasoning_effort,
                facts.reflection_notice,
                i64_value(now_ms)?,
            ],
        )
        .map_err(storage)?;
    Ok(())
}

/// 补写某一轮的反思提示。
///
/// 收口那一笔已经写过整行，这里只改一个字段；行不存在时不插入（读侧会回落尾窗读），
/// 因此不会用一条只有提示、其余全空的半截事实盖住真值。
pub(super) fn update_turn_notice(
    store: &SqliteRuntimeStore,
    thread_id: &str,
    assistant_part_id: &str,
    notice: &str,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let connection = store.connection()?;
    connection
        .execute(
            "UPDATE thread_turn_facts
                SET reflection_notice = ?3, updated_at_ms = ?4
              WHERE thread_id = ?1 AND assistant_part_id = ?2",
            params![thread_id, assistant_part_id, notice, i64_value(now_ms)?,],
        )
        .map_err(storage)?;
    Ok(())
}

/// 按 `(thread, 轮次)` 点查一轮的事实；没有这一行时为空。
pub(super) fn turn_facts(
    store: &SqliteRuntimeStore,
    thread_id: &str,
    assistant_part_id: &str,
) -> Result<Option<ThreadTurnFacts>, RuntimeStoreError> {
    let connection = store.connection()?;
    connection
        .query_row(
            &format!(
                "SELECT {FACT_COLUMNS}
                   FROM thread_turn_facts
                  WHERE thread_id = ?1 AND assistant_part_id = ?2"
            ),
            params![thread_id, assistant_part_id],
            decode_row,
        )
        .optional()
        .map_err(storage)
}

/// 读一条 thread 最新一轮的事实；没有这一行时为空。
pub(super) fn latest_turn_facts(
    store: &SqliteRuntimeStore,
    thread_id: &str,
) -> Result<Option<ThreadTurnFacts>, RuntimeStoreError> {
    let connection = store.connection()?;
    connection
        .query_row(
            &format!(
                "SELECT {FACT_COLUMNS}
                   FROM thread_turn_facts
                  WHERE thread_id = ?1
                  ORDER BY recorded_at_ms DESC, assistant_part_id DESC
                  LIMIT 1"
            ),
            [thread_id],
            decode_row,
        )
        .optional()
        .map_err(storage)
}
