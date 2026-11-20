//! 逐轮事实投影：收口那一批里把答案、读数与工具名落成一行。
//!
//! 索引页原本每次都要扫事件日志尾部才凑出这三样；这里在收口时付一次成本（只扫这一轮
//! 自己的帧，且有帧数上限），之后翻页是一次按会话的区间查询。

use axon_domain::DaemonSessionId;

use super::SqliteRuntimeStore;
use super::turn_activity_repository::{TurnActivity, upsert_activity};

/// 工具名扫描的帧上限：一轮的工具调用远小于这个数，超出只取前面的部分。
const TOOL_SCAN_FRAME_LIMIT: u32 = 4_000;

/// 从收口批次里投影一行逐轮事实。
///
/// 只认 `turn_completed` 与 `turn_metrics`：这两条保证同批落盘（终态与它派生的指标），
/// 因此一次调用就能把答案与读数一起写进同一行。
pub(super) fn project_turn_activity(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) {
    let mut terminal: Option<(String, Option<String>)> = None;
    let mut metrics_json: Option<String> = None;
    for (event_type, payload_json) in events {
        match event_type.as_str() {
            "turn_completed" => {
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
                let response = payload
                    .get("response")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned);
                terminal = Some((interaction_id.to_owned(), response));
            }
            "turn_metrics" => metrics_json = Some(payload_json.clone()),
            _ => {}
        }
    }
    let Some((interaction_id, answer_text)) = terminal else {
        return;
    };
    let tool_names = tool_names_for(store, session_id, &interaction_id);
    let activity = TurnActivity {
        session_id: session_id.as_str().to_owned(),
        interaction_id,
        // 排序不靠这一列（读侧按 interaction 主键取），落收口时刻便于排查。
        received_at_ms: now_ms,
        answer_text,
        tool_names,
        metrics_json,
    };
    if let Err(error) = upsert_activity(store, &activity, now_ms) {
        eprintln!(
            "逐轮事实投影写入失败，可回落日志尾扫: session={} error={error}",
            session_id.as_str()
        );
    }
}

/// 这一轮用过哪些工具：只扫它自己的帧，且有帧数上限。
fn tool_names_for(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    interaction_id: &str,
) -> Vec<String> {
    let path = store.event_log(session_id).path();
    if !path.exists() {
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
