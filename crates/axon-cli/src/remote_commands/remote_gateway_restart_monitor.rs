use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_domain::SessionId;
use axon_jsonl::ShardedJsonlStore;
use axon_remote_feishu::RemoteFeishuAgentReplyArtifactRuntime;
use serde::{Deserialize, Serialize};

use crate::run_commands::{RemoteAgentExecutionInput, enqueue_remote_gateway_alert};

use super::{RemoteCliContext, remote_feishu_gateway_status::GatewayRuntimeStatus};

const RESTART_STATE_FILE: &str = "gateway-restart-state.json";
const RESTART_WINDOW_MS: u64 = 5 * 60 * 1_000;
const RESTART_THRESHOLD: usize = 3;
const ALERT_COOLDOWN_MS: u64 = 30 * 60 * 1_000;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GatewayRestartState {
    #[serde(default)]
    starts_ms: Vec<u64>,
    #[serde(default)]
    last_alert_ms: Option<u64>,
    #[serde(default)]
    last_pid: Option<u32>,
}

pub(super) fn record_gateway_start(
    context: &RemoteCliContext,
    event_file: &Path,
    pid: u32,
) -> Result<(), String> {
    let status_file = context.root.join("gateway-status.json");
    record_gateway_start_at(context, event_file, &status_file, pid, super::now_ms()).map(|_| ())
}

fn record_gateway_start_at(
    context: &RemoteCliContext,
    event_file: &Path,
    status_file: &Path,
    pid: u32,
    now_ms: u64,
) -> Result<bool, String> {
    let state_path = context.root.join(RESTART_STATE_FILE);
    let mut state = read_state(&state_path)?;
    if state.last_pid == Some(pid) {
        return Ok(false);
    }
    let window_start = now_ms.saturating_sub(RESTART_WINDOW_MS);
    state.starts_ms.retain(|started| *started >= window_start);
    state.starts_ms.push(now_ms);
    state.last_pid = Some(pid);

    let cooldown_elapsed = state
        .last_alert_ms
        .is_none_or(|alerted| now_ms.saturating_sub(alerted) >= ALERT_COOLDOWN_MS);
    let should_alert = state.starts_ms.len() >= RESTART_THRESHOLD && cooldown_elapsed;
    let alerted = if should_alert {
        enqueue_restart_alert(
            context,
            event_file,
            status_file,
            pid,
            state.starts_ms.len(),
            now_ms,
        )?
    } else {
        false
    };
    if alerted {
        state.last_alert_ms = Some(now_ms);
    }
    write_state(&state_path, &state)?;
    Ok(alerted)
}

fn enqueue_restart_alert(
    context: &RemoteCliContext,
    event_file: &Path,
    status_file: &Path,
    pid: u32,
    restart_count: usize,
    now_ms: u64,
) -> Result<bool, String> {
    let Some(chat_id) = latest_chat_id(context, event_file)? else {
        return Ok(false);
    };
    let last_error = GatewayRuntimeStatus::read(status_file)
        .and_then(|status| status.last_error)
        .filter(|error| !error.trim().is_empty())
        .unwrap_or_else(|| "无已记录错误".to_owned());
    let body = format!(
        "Axon daemon 重启告警\n\n检测到 5 分钟内启动 {restart_count} 次。\n当前 PID: {pid}\n最近错误: {last_error}\n请运行 `axon daemon status` 查看服务状态。"
    );
    let delivery_id = format!("gateway-restart-alert-{now_ms}");
    enqueue_remote_gateway_alert(
        &RemoteAgentExecutionInput {
            axon_root: &context.root,
            axon_session_id: SessionId(context.gateway_session_id()),
            run_session_key: "gateway-restart-monitor",
            feishu_chat_id: &chat_id,
            processing_message_id: None,
            reply_to_message_id: None,
            thread_id: None,
            pairing_id: "feishu-gateway-restart-monitor",
            delivery_id: &delivery_id,
            recorded_at_ms: now_ms,
        },
        &body,
    )?;
    Ok(true)
}

fn latest_chat_id(context: &RemoteCliContext, event_file: &Path) -> Result<Option<String>, String> {
    let artifact_path = RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&context.root);
    let latest_artifact = RemoteFeishuAgentReplyArtifactRuntime::read_records(artifact_path)
        .map_err(|error| format!("读取 Feishu 最近回复目标失败: {error:?}"))?
        .into_iter()
        .filter(|record| !record.receive_id.trim().is_empty())
        .max_by_key(|record| record.recorded_at_ms)
        .map(|record| record.receive_id);
    if latest_artifact.is_some() {
        return Ok(latest_artifact);
    }
    latest_event_chat_id(event_file)
}

fn latest_event_chat_id(path: &Path) -> Result<Option<String>, String> {
    let lines = ShardedJsonlStore::new()
        .read_lines(path)
        .map_err(|error| format!("读取 Feishu gateway 事件目标失败: {error}"))?;
    Ok(lines.iter().rev().find_map(|line| {
        let value = serde_json::from_slice::<serde_json::Value>(line).ok()?;
        value
            .get("chatId")
            .or_else(|| value.get("chat_id"))
            .and_then(serde_json::Value::as_str)
            .filter(|chat_id| !chat_id.trim().is_empty())
            .map(ToOwned::to_owned)
    }))
}

fn read_state(path: &Path) -> Result<GatewayRestartState, String> {
    if !path.exists() {
        return Ok(GatewayRestartState::default());
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("读取 gateway 重启状态失败: {error}"))?;
    serde_json::from_str(&content).map_err(|error| format!("gateway 重启状态 JSON 无效: {error}"))
}

fn write_state(path: &Path, state: &GatewayRestartState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 gateway 重启状态目录失败: {error}"))?;
    }
    let temp_path = temporary_path(path);
    let content = serde_json::to_vec_pretty(state)
        .map_err(|error| format!("序列化 gateway 重启状态失败: {error}"))?;
    fs::write(&temp_path, content)
        .map_err(|error| format!("写入 gateway 重启临时状态失败: {error}"))?;
    fs::rename(&temp_path, path).map_err(|error| format!("更新 gateway 重启状态失败: {error}"))
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!("tmp-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_config::RemoteFeishuWebSocketConfig;
    use axon_jsonl::ShardedJsonlStore;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_context(root: &Path) -> RemoteCliContext {
        RemoteCliContext::new(
            root.join("config.toml"),
            root.to_path_buf(),
            RemoteFeishuWebSocketConfig::default(),
        )
        .feishu_runtime()
    }

    fn temp_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!("axon-{name}-{}-{nonce}", std::process::id()))
    }

    #[test]
    fn alerts_on_third_start_and_respects_cooldown() {
        let root = temp_root("restart-alert");
        let context = test_context(&root);
        fs::create_dir_all(&context.root).expect("runtime root");
        let event_file = context.root.join("events.jsonl");
        fs::write(
            ShardedJsonlStore::physical_shard_path(&event_file, 0),
            "{\"chatId\":\"oc_alert\"}\n",
        )
        .expect("event");
        let status_file = context.root.join("gateway-status.json");

        assert!(!record_gateway_start_at(&context, &event_file, &status_file, 1, 1_000).unwrap());
        assert!(!record_gateway_start_at(&context, &event_file, &status_file, 2, 2_000).unwrap());
        assert!(record_gateway_start_at(&context, &event_file, &status_file, 3, 3_000).unwrap());
        assert!(!record_gateway_start_at(&context, &event_file, &status_file, 4, 4_000).unwrap());

        let artifacts = RemoteFeishuAgentReplyArtifactRuntime::read_records(
            RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&context.root),
        )
        .expect("artifacts");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].receive_id, "oc_alert");
        assert!(artifacts[0].body.contains("5 分钟内启动 3 次"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn does_not_consume_cooldown_without_a_delivery_target() {
        let root = temp_root("restart-no-target");
        let context = test_context(&root);
        fs::create_dir_all(&context.root).expect("runtime root");
        let event_file = context.root.join("missing-events.jsonl");
        let status_file = context.root.join("gateway-status.json");

        for (pid, now) in [(1, 1_000), (2, 2_000), (3, 3_000)] {
            assert!(
                !record_gateway_start_at(&context, &event_file, &status_file, pid, now).unwrap()
            );
        }
        let state = read_state(&context.root.join(RESTART_STATE_FILE)).expect("state");
        assert_eq!(state.last_alert_ms, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reconnects_in_the_same_process_are_not_counted_as_restarts() {
        let root = temp_root("restart-same-pid");
        let context = test_context(&root);
        fs::create_dir_all(&context.root).expect("runtime root");
        let event_file = context.root.join("events.jsonl");
        fs::write(
            ShardedJsonlStore::physical_shard_path(&event_file, 0),
            "{\"chatId\":\"oc_alert\"}\n",
        )
        .expect("event");
        let status_file = context.root.join("gateway-status.json");

        assert!(!record_gateway_start_at(&context, &event_file, &status_file, 7, 1_000).unwrap());
        assert!(!record_gateway_start_at(&context, &event_file, &status_file, 7, 2_000).unwrap());
        assert!(!record_gateway_start_at(&context, &event_file, &status_file, 7, 3_000).unwrap());

        let state = read_state(&context.root.join(RESTART_STATE_FILE)).expect("state");
        assert_eq!(state.starts_ms, vec![1_000]);
        let _ = fs::remove_dir_all(root);
    }
}
