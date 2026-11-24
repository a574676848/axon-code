//! 逐轮事实投影：索引页要的答案、工具名与读数在收口时落一行。
//!
//! 为什么需要它：索引页原先靠一次事件日志尾扫（最多 16384 帧）凑出每轮答案与工具名，
//! 成本随会话增长。这里在收口时写一行，翻页变成一次按会话的区间查询。
//!
//! 表是投影：写失败不阻断收口，读侧缺行时仍回落日志尾扫。

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use super::SqliteRuntimeStore;
use super::helpers::{i64_value, storage};
use crate::ports::RuntimeStoreError;

/// 一轮的活动事实。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnActivity {
    /// 车道会话标识。
    pub session_id: String,
    /// 该轮交互标识。
    pub interaction_id: String,
    /// 入站接收时刻，与索引页的排序键一致。
    pub received_at_ms: u64,
    /// 最终交付正文；没有收口正文时为空。
    pub answer_text: Option<String>,
    /// 本轮用过的工具名，按出现顺序去重。
    pub tool_names: Vec<String>,
    /// 本轮读数（`TurnMetrics` 的 JSON 原文）。
    pub metrics_json: Option<String>,
}

/// 写入或更新一轮的活动事实。
pub(super) fn upsert_activity(
    store: &SqliteRuntimeStore,
    activity: &TurnActivity,
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let connection = store.connection()?;
    connection
        .execute(
            "INSERT INTO turn_activity (
                 session_id, interaction_id, received_at_ms, answer_text, tool_names_json,
                 metrics_json, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(session_id, interaction_id) DO UPDATE SET
                 received_at_ms = excluded.received_at_ms,
                 answer_text = COALESCE(excluded.answer_text, turn_activity.answer_text),
                 tool_names_json = COALESCE(excluded.tool_names_json, turn_activity.tool_names_json),
                 metrics_json = COALESCE(excluded.metrics_json, turn_activity.metrics_json),
                 updated_at_ms = excluded.updated_at_ms",
            params![
                activity.session_id,
                activity.interaction_id,
                i64_value(activity.received_at_ms)?,
                activity.answer_text,
                if activity.tool_names.is_empty() {
                    None
                } else {
                    Some(
                        serde_json::to_string(&activity.tool_names)
                            .map_err(|error| storage_msg(&error.to_string()))?,
                    )
                },
                activity.metrics_json,
                i64_value(now_ms)?,
            ],
        )
        .map_err(storage)?;
    Ok(())
}

/// 一批交互的逐轮事实；不在表里的轮次不出现在结果中。
pub(super) fn activities_for(
    store: &SqliteRuntimeStore,
    session_id: &str,
    interaction_ids: &[&str],
) -> Result<HashMap<String, TurnActivity>, RuntimeStoreError> {
    let mut found = HashMap::new();
    if interaction_ids.is_empty() {
        return Ok(found);
    }
    let connection = store.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT interaction_id, received_at_ms, answer_text, tool_names_json, metrics_json
               FROM turn_activity
              WHERE session_id = ?1 AND interaction_id = ?2",
        )
        .map_err(storage)?;
    for interaction_id in interaction_ids {
        let row = statement
            .query_row(params![session_id, *interaction_id], |row| {
                Ok((
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .optional()
            .map_err(storage)?;
        let Some((received_at_ms, answer_text, tool_names_json, metrics_json)) = row else {
            continue;
        };
        found.insert(
            (*interaction_id).to_owned(),
            TurnActivity {
                session_id: session_id.to_owned(),
                interaction_id: (*interaction_id).to_owned(),
                received_at_ms: u64::try_from(received_at_ms).unwrap_or(0),
                answer_text,
                tool_names: tool_names_json
                    .as_deref()
                    .and_then(|text| serde_json::from_str(text).ok())
                    .unwrap_or_default(),
                metrics_json,
            },
        );
    }
    Ok(found)
}

fn storage_msg(message: &str) -> RuntimeStoreError {
    RuntimeStoreError::Storage(message.to_owned())
}
