//! 逐轮事实投影：收口那一批里把答案、读数与工具名落成一行。
//!
//! 索引页原本每次都要扫事件日志尾部才凑出这三样；这里在收口时付一次成本，之后翻页是
//! 一次按会话的区间查询。工具名由事件 sink 在投递每一帧时累积、收口时挂到终态帧上，
//! 因此投影路径不再解压任何事件日志帧：缺 `toolNames` 的老帧按「没有工具调用」落库。
//!
//! 老记录（升级前收口的轮次）本来就已经投影过，不受这条影响。

use axon_domain::DaemonSessionId;

use super::SqliteRuntimeStore;
use super::turn_activity_repository::{TurnActivity, upsert_activity};
use crate::ports::RuntimeStoreError;

/// 从收口批次里投影一行逐轮事实。
///
/// 只认 `turn_completed` 与 `turn_metrics`：这两条保证同批落盘（终态与它派生的指标），
/// 因此一次调用就能把答案与读数一起写进同一行。
///
/// 写入失败原样返回错误：水位只在三个投影都成功后推进，失败批次留给重放或重建入口。
pub(super) fn project_turn_activity(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
    events: &[(String, String)],
    now_ms: u64,
) -> Result<(), RuntimeStoreError> {
    let mut terminal: Option<(String, Option<String>)> = None;
    let mut terminal_tool_names: Option<Vec<String>> = None;
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
                // 收口批里带了本轮工具名时直接用：写路径不再回扫这一轮的帧。
                if let Some(names) = payload
                    .get("toolNames")
                    .and_then(serde_json::Value::as_array)
                {
                    let names = names
                        .iter()
                        .filter_map(serde_json::Value::as_str)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>();
                    if !names.is_empty() {
                        terminal_tool_names = Some(names);
                    }
                }
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
        return Ok(());
    };
    // 老帧没有 toolNames 字段：按没有工具调用落库。写路径不再回扫这一轮的帧，
    // 投影路径的日志解压次数因此是 0；升级前收口的轮次早已投影过，不受影响。
    let tool_names = terminal_tool_names.unwrap_or_default();
    let activity = TurnActivity {
        session_id: session_id.as_str().to_owned(),
        interaction_id,
        // 排序不靠这一列（读侧按 interaction 主键取），落收口时刻便于排查。
        received_at_ms: now_ms,
        answer_text,
        tool_names,
        metrics_json,
    };
    upsert_activity(store, &activity, now_ms)
}
