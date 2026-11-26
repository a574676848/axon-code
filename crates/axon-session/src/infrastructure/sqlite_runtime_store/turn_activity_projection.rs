//! 逐轮事实投影：收口那一批里把答案、读数与工具名落成一行。
//!
//! 索引页原本每次都要扫事件日志尾部才凑出这三样；这里在收口时付一次成本（只扫这一轮
//! 自己的帧，且有帧数上限），之后翻页是一次按会话的区间查询。

use axon_domain::DaemonSessionId;

use super::SqliteRuntimeStore;
use super::turn_activity_repository::{TurnActivity, upsert_activity};
use crate::ports::RuntimeStoreError;

/// 工具名扫描的帧上限：一轮的工具调用远小于这个数，超出只取前面的部分。
const TOOL_SCAN_FRAME_LIMIT: u32 = 4_000;

/// 准备一批事件里的逐轮事实。
///
/// 解析、缺工具名时的日志回扫都在这里完成；写事务里只做 upsert，不碰文件。一批里可能
/// 有多轮（重建会把连续的多轮读进同一批），因此按 `turn_completed` 与其后的
/// `turn_metrics` 逐对配对，而不是只取最后一条。
pub(super) fn prepare_turn_activities(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<Vec<TurnActivity>, RuntimeStoreError> {
    struct Pending {
        interaction_id: String,
        answer_text: Option<String>,
        tool_names: Option<Vec<String>>,
        metrics_json: Option<String>,
    }

    let mut rows: Vec<Pending> = Vec::new();
    let mut current: Option<Pending> = None;
    for (event_type, payload_json) in events {
        match event_type.as_str() {
            "turn_completed" => {
                if let Some(done) = current.take() {
                    rows.push(done);
                }
                let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_json) else {
                    continue;
                };
                let Some(interaction_id) = payload
                    .get("interactionId")
                    .and_then(serde_json::Value::as_str)
                    .filter(|value| !value.is_empty())
                else {
                    continue;
                };
                let tool_names = payload
                    .get("toolNames")
                    .and_then(serde_json::Value::as_array)
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .filter(|name| !name.is_empty())
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .filter(|names: &Vec<String>| !names.is_empty());
                let answer_text = payload
                    .get("response")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned);
                current = Some(Pending {
                    interaction_id: interaction_id.to_owned(),
                    answer_text,
                    tool_names,
                    metrics_json: None,
                });
            }
            "turn_metrics" => {
                // 指标属于最近一条终态：两条本来就同批落盘，只有重建才会把多轮并进一批。
                if let Some(row) = current.as_mut() {
                    row.metrics_json = Some(payload_json.clone());
                }
            }
            _ => {}
        }
    }
    if let Some(done) = current.take() {
        rows.push(done);
    }

    let mut activities = Vec::with_capacity(rows.len());
    for row in rows {
        // 老记录没有 toolNames 字段：退回按这一次交互的有界扫描（有上限、且要求索引可用）。
        let tool_names = row
            .tool_names
            .unwrap_or_else(|| tool_names_for(store, session_id, &row.interaction_id));
        activities.push(TurnActivity {
            session_id: session_id.as_str().to_owned(),
            interaction_id: row.interaction_id,
            // 排序不靠这一列（读侧按 interaction 主键取），落收口时刻便于排查。
            received_at_ms: now_ms,
            answer_text: row.answer_text,
            tool_names,
            metrics_json: row.metrics_json,
        });
    }
    Ok(activities)
}

/// 在调用方的事务里写入准备好的逐轮事实；与快照、累计指标共用一次提交。
pub(super) fn write_turn_activities(
    connection: &rusqlite::Connection,
    activities: &[TurnActivity],
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    for activity in activities {
        upsert_activity(connection, activity, now_ms)?;
    }
    Ok(())
}

/// 单测入口：准备并独立提交一批逐轮事实。
#[cfg(test)]
pub(super) fn project_turn_activity(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let activities = prepare_turn_activities(store, session_id, events, now_ms)?;
    let mut connection = store.connection()?;
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|error| RuntimeStoreError::Storage(error.to_string()))?;
    write_turn_activities(&transaction, &activities, now_ms)?;
    transaction
        .commit()
        .map_err(|error| RuntimeStoreError::Storage(error.to_string()))
}

/// 这一轮用过哪些工具：只扫它自己的帧，且有帧数上限。
fn tool_names_for(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &str,
) -> Vec<String> {
    let path = store.event_log(session_id).path();
    if !path.exists() || !axon_eventlog::index_is_fresh(&path) {
        return Vec::new();
    }
    let Ok(records) = axon_eventlog::read_interaction_after(
        &path,
        interaction_id,
        0,
        TOOL_SCAN_FRAME_LIMIT as usize,
    ) else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for record in records {
        let Some(name) = record
            .payload
            .get("kind")
            .and_then(|kind| kind.get("payload"))
            .filter(|_| {
                record
                    .payload
                    .get("kind")
                    .and_then(|kind| kind.get("type"))
                    .and_then(serde_json::Value::as_str)
                    == Some("toolCall")
            })
            .and_then(|call| call.get("toolName"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if !names.iter().any(|known| known == name) {
            names.push(name.to_owned());
        }
    }
    names
}
