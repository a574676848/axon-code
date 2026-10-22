//! TUI 启动时的最近完成回合历史读取。

use std::path::Path;

use axon_domain::{
    AudienceContext, AudienceKind, DaemonSessionId, DisclosurePolicy, InteractionRecord,
};
use axon_protocol::wire::{WireEventEnvelope, WireEventKind};
use axon_session::{
    InteractionQueryStore, RuntimeStore, SessionEventRecord, SqliteRuntimeStore, session_lane_id,
};
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
    let lane_id = session_lane_id(
        &workspace.workspace_id,
        &AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly),
        interaction.reply_route.channel(),
        interaction.reply_route.destination(),
    );
    let session_id = DaemonSessionId::new(lane_id)
        .map_err(|error| format!("解析 Daemon 历史 Session ID 失败：{error}"))?;
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
