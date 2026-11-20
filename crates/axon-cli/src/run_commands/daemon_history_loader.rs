//! TUI 启动时的最近完成回合历史读取。

use axon_session::EventLogStore as _;
use std::path::Path;

use axon_domain::{DaemonSessionId, InteractionRecord};
use axon_protocol::wire::{WireEventEnvelope, WireEventKind};
use axon_session::{InteractionQueryStore, SessionEventRecord, SqliteRuntimeStore};
use serde::Deserialize;

const TURN_COMPLETED_EVENT: &str = "turn_completed";
const WIRE_EVENT: &str = "wire_event";
const HISTORY_REPLAY_PAGE_SIZE: u32 = 100;

#[derive(Debug)]
pub(crate) struct LatestCompletedTurn {
    pub(crate) session_id: String,
    pub(crate) response: String,
    pub(crate) wire_events: Vec<WireEventEnvelope>,
}

pub(crate) fn load_active_interaction(
    cwd: &Path,
    config_path: Option<&Path>,
    channel: &str,
    destination: &str,
) -> Result<Option<InteractionRecord>, String> {
    let workspace = super::register_workspace(cwd, axon_time::unix_now_ms(), config_path)?;
    let store = SqliteRuntimeStore::from_root(runtime_data_root(config_path)?);
    let interaction = store
        .latest_processing_interaction_for_route(
            workspace.workspace_id.as_str(),
            channel,
            destination,
        )
        .map_err(|error| format!("读取 Daemon 活动 Interaction 失败：{error}"))?;
    Ok(interaction)
}

pub(crate) fn load_active_interaction_for_workspace(
    cwd: &Path,
    config_path: Option<&Path>,
    channel: &str,
) -> Result<Option<InteractionRecord>, String> {
    let workspace = super::register_workspace(cwd, axon_time::unix_now_ms(), config_path)?;
    let store = SqliteRuntimeStore::from_root(runtime_data_root(config_path)?);
    store
        .latest_processing_interaction_for_channel(workspace.workspace_id.as_str(), channel)
        .map_err(|error| format!("读取 Workspace 活动 Interaction 失败：{error}"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnCompletedPayload {
    response: String,
}

pub(crate) fn load_latest_completed_turn_for_workspace(
    cwd: &Path,
    config_path: Option<&Path>,
    channel: &str,
) -> Result<Option<LatestCompletedTurn>, String> {
    let workspace = super::register_workspace(cwd, axon_time::unix_now_ms(), config_path)?;
    let store = SqliteRuntimeStore::from_root(runtime_data_root(config_path)?);
    let Some(interaction) = store
        .latest_external_interaction_for_channel(workspace.workspace_id.as_str(), channel)
        .map_err(|error| format!("读取 Workspace 最近 Interaction 失败：{error}"))?
    else {
        return Ok(None);
    };
    // 会话标识是入站事实的一列：直接读最近那条 Interaction 的会话，不再按投递目标派生。
    let session_id = interaction.session_id.clone();
    load_completed_turn_from_store(&store, &session_id)
}

fn load_completed_turn_from_store(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Result<Option<LatestCompletedTurn>, String> {
    let records = replay_all_events(store, session_id)?;
    let mut candidates =
        std::collections::BTreeMap::<String, (u64, String, Vec<WireEventEnvelope>)>::new();
    for record in records {
        let Some(interaction_id) = payload_interaction_id(&record.payload_json) else {
            continue;
        };
        let entry = candidates
            .entry(interaction_id)
            .or_insert_with(|| (0, String::new(), Vec::new()));
        match record.event_type.as_str() {
            WIRE_EVENT => {
                let wire = serde_json::from_str::<WireEventEnvelope>(&record.payload_json)
                    .map_err(|error| format!("解析历史 wire event 失败：{error}"))?;
                entry.2.push(wire);
            }
            TURN_COMPLETED_EVENT => {
                let completed = serde_json::from_str::<TurnCompletedPayload>(&record.payload_json)
                    .map_err(|error| format!("解析历史 turn_completed 事件失败：{error}"))?;
                entry.0 = record.sequence;
                entry.1 = completed.response;
            }
            _ => {}
        }
    }
    let Some((_, (_, response, mut wire_events))) = candidates
        .into_iter()
        .filter(|(_, (_, response, _))| !response.trim().is_empty())
        .max_by_key(|(_, (sequence, _, _))| *sequence)
    else {
        return Ok(None);
    };
    wire_events.sort_by_key(|event| event.sequence);
    if !wire_events.iter().any(|event| {
        matches!(
            &event.kind,
            WireEventKind::AssistantMessage(message) if !message.delta
        )
    }) {
        return Err("Daemon 完成回合缺少 durable canonical assistant".to_owned());
    }
    Ok(Some(LatestCompletedTurn {
        session_id: session_id.to_string(),
        response,
        wire_events,
    }))
}

/// 一条会话里按时间排好的一轮 durable wire 事件。
pub(crate) struct SessionHistoryTurn {
    pub(crate) wire_events: Vec<WireEventEnvelope>,
}

/// 读取一条会话的全部轮次（按轮次发生顺序）。
///
/// 与「最近完成回合」同源：只有 durable 会话事件是回放事实，正文、工具回执与用量
/// 都在这些帧里，因此 `/sessions` 选中任意一条会话都能还原它当初的样子。
pub(crate) fn load_session_turns(
    config_path: Option<&Path>,
    session_id: &str,
) -> Result<Vec<SessionHistoryTurn>, String> {
    let store = SqliteRuntimeStore::from_root(runtime_data_root(config_path)?);
    let session = DaemonSessionId::new(session_id.to_owned()).map_err(|error| error.to_string())?;
    let records = replay_all_events(&store, &session)?;
    group_turns(records)
}

/// 把一条会话的 durable 事件按 Interaction 归并成轮次，保持事件里的先后顺序。
fn group_turns(records: Vec<SessionEventRecord>) -> Result<Vec<SessionHistoryTurn>, String> {
    let mut order = Vec::<String>::new();
    let mut by_interaction = std::collections::HashMap::<String, Vec<WireEventEnvelope>>::new();
    for record in records {
        if record.event_type != WIRE_EVENT {
            continue;
        }
        let Some(interaction_id) = payload_interaction_id(&record.payload_json) else {
            continue;
        };
        let wire = serde_json::from_str::<WireEventEnvelope>(&record.payload_json)
            .map_err(|error| format!("解析历史 wire event 失败：{error}"))?;
        if !by_interaction.contains_key(&interaction_id) {
            order.push(interaction_id.clone());
        }
        by_interaction.entry(interaction_id).or_default().push(wire);
    }

    Ok(order
        .into_iter()
        .filter_map(|interaction_id| by_interaction.remove(&interaction_id))
        .map(|mut wire_events| {
            wire_events.sort_by_key(|event| event.sequence);
            SessionHistoryTurn { wire_events }
        })
        .collect())
}

fn runtime_data_root(config_path: Option<&Path>) -> Result<std::path::PathBuf, String> {
    match config_path {
        Some(path) => {
            crate::runtime_paths::runtime_axon_data_path_static(path).map_err(str::to_owned)
        }
        None => {
            let path = axon_config::ConfigLoader::default_user_config_path()
                .map_err(|_| "无法解析用户配置路径".to_owned())?;
            crate::runtime_paths::runtime_axon_data_path_static(&path).map_err(str::to_owned)
        }
    }
}

fn replay_all_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Result<Vec<SessionEventRecord>, String> {
    let mut records = Vec::new();
    let mut after_sequence = 0;
    loop {
        let page = store
            .replay_events(session_id, after_sequence, HISTORY_REPLAY_PAGE_SIZE)
            .map_err(|error| format!("读取 Daemon 历史事件失败：{error}"))?;
        let Some(last) = page.last() else {
            break;
        };
        after_sequence = last.sequence;
        let page_len = page.len();
        records.extend(page);
        if page_len < HISTORY_REPLAY_PAGE_SIZE as usize {
            break;
        }
    }
    Ok(records)
}

fn payload_interaction_id(payload_json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(payload_json)
        .ok()?
        .get("interactionId")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::DaemonSessionId;

    fn wire_record(sequence: u64, interaction_id: &str, content: &str) -> SessionEventRecord {
        let wire = WireEventEnvelope {
            schema_version: axon_protocol::wire::WireSchemaVersion::default(),
            event_id: axon_protocol::wire::WireCorrelationId(format!("event:{sequence}")),
            correlation_id: axon_protocol::wire::WireCorrelationId("turn".to_owned()),
            session: axon_protocol::wire::WireSessionRef {
                session_id: "session-1".to_owned(),
                run_id: None,
                parent_session_id: None,
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap_or(0),
            kind: WireEventKind::AssistantMessage(axon_protocol::wire::WireAssistantMessage {
                part_id: format!("assistant:{sequence}"),
                message_id: format!("message:{sequence}"),
                content: content.to_owned(),
                delta: false,
            }),
        };
        let mut payload = serde_json::to_value(&wire).expect("序列化 wire");
        payload["interactionId"] = serde_json::Value::String(interaction_id.to_owned());
        SessionEventRecord {
            session_id: DaemonSessionId::new("session-1").expect("会话 ID"),
            sequence,
            generation: "generation-1".to_owned(),
            event_type: WIRE_EVENT.to_owned(),
            payload_json: payload.to_string(),
            emitted_at_ms: sequence,
        }
    }

    #[test]
    fn session_turns_group_events_by_interaction_in_arrival_order() {
        // 交错的顺序（A1、B1、A2）也必须按轮次归并，且轮次内部按 sequence 排序。
        let records = vec![
            wire_record(3, "interaction-a", "第二轮补充"),
            wire_record(1, "interaction-a", "第一轮正文"),
            wire_record(2, "interaction-b", "第二轮正文"),
        ];

        let turns = group_turns(records).expect("分组不应失败");

        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].wire_events.len(), 2);
        assert_eq!(turns[0].wire_events[0].sequence, 1);
        assert_eq!(turns[0].wire_events[1].sequence, 3);
        assert_eq!(turns[1].wire_events[0].sequence, 2);
    }

    #[test]
    fn session_turns_skip_records_without_an_interaction() {
        let mut record = wire_record(1, "interaction-a", "正文");
        record.payload_json = "{\"sequence\":1}".to_owned();

        let turns = group_turns(vec![record]).expect("分组不应失败");

        assert!(turns.is_empty(), "没有归属轮的帧不属于任何一轮");
    }
}
