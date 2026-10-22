use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::super::RemoteGatewayCommand;
use axon_jsonl::ShardedJsonlStore;
use axon_remote_feishu::{
    REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS, RemoteFeishuGatewayEventSourceWriterRuntime,
};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub(super) struct RemoteFeishuGatewayCursorState {
    pub(super) acked_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FeishuGatewayInputEvent {
    #[serde(default = "default_event_kind")]
    pub(super) kind: String,
    pub(super) request_id: String,
    pub(super) chat_id: String,
    #[serde(default)]
    pub(super) chat_type: Option<String>,
    #[serde(default)]
    pub(super) message_id: Option<String>,
    #[serde(default)]
    pub(super) source_created_at_ms: Option<u64>,
    #[serde(default)]
    pub(super) sender_open_id: Option<String>,
    #[serde(default)]
    pub(super) operator_open_id: Option<String>,
    #[serde(default)]
    pub(super) mentions: Vec<String>,
    #[serde(default)]
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) media_refs: Vec<RemoteFeishuGatewayMediaRefInput>,
    #[serde(default)]
    pub(super) message_kind: Option<String>,
    #[serde(default)]
    pub(super) action_tag: Option<String>,
    #[serde(default)]
    pub(super) action_value_json: Option<String>,
    #[serde(default)]
    pub(super) reply_context: Option<RemoteFeishuGatewayReplyContextInput>,
    #[serde(default)]
    pub(super) session_busy: bool,
    #[serde(default)]
    pub(super) enqueued_at_ms: Option<u64>,
    #[serde(default)]
    pub(super) raw_payload_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuGatewayMediaRefInput {
    pub(super) kind: String,
    pub(super) key: String,
    #[serde(default)]
    pub(super) file_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RemoteFeishuGatewayReplyContextInput {
    #[serde(default)]
    pub(super) reply_to_message_id: Option<String>,
    #[serde(default)]
    pub(super) reply_to_text: Option<String>,
    #[serde(default)]
    pub(super) thread_id: Option<String>,
}

/// 从给定行偏移起读取至多 `max_events` 条完整事件，并返回扫描结束后的行偏移。
///
/// 契约：
/// - `acked_offset` 是调用方的**读偏移**，与 durable 确认偏移解耦（车道调度器分别持有
///   两者：读偏移随投递前进，确认偏移只随「整批完成」的连续前缀前进）。
/// - 已完整换行但反序列化失败（含无效 UTF-8）的记录视为已消费，读偏移照常前进；
///   未换行的尾记录不消费。整批都是坏行时返回空事件集，调用方仍须推进偏移与确认前缀。
/// - 单次调用最多反序列化 `max_events` 条事件，但需要先读取偏移之前的全部行；因此
///   调用频率由未确认行窗口（车道调度的背压）约束，避免在长 backlog 上反复全量解析。
pub(super) fn read_gateway_input_events_after_cursor(
    path: &Path,
    acked_offset: u64,
    max_events: usize,
) -> Result<(Vec<FeishuGatewayInputEvent>, u64), String> {
    if max_events == 0 || max_events > REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS {
        return Err("Feishu gateway 单轮事件读取上限无效".to_owned());
    }
    let lines = ShardedJsonlStore::new()
        .read_repaired_lines(path)
        .map_err(|error| format!("读取 Feishu gateway 事件源失败: {error}"))?;
    if acked_offset > lines.len() as u64 {
        return Err("Feishu gateway cursor 行偏移超出事件源长度".to_owned());
    }
    let mut events = Vec::new();
    let mut next_acked_offset = acked_offset;
    for line in lines
        .iter()
        .skip(usize::try_from(acked_offset).unwrap_or(usize::MAX))
    {
        if events.len() >= max_events {
            break;
        }
        next_acked_offset = next_acked_offset.saturating_add(1);
        if let Ok(event) = serde_json::from_slice::<FeishuGatewayInputEvent>(line) {
            events.push(event);
        }
    }
    Ok((events, next_acked_offset))
}

/// 仅在 cursor 已确认完整事件源且没有新 append 时清空已消费数据。
pub(super) fn reset_gateway_event_source_if_fully_acknowledged(
    event_source_path: &Path,
    cursor_path: &Path,
    cursor: &mut RemoteFeishuGatewayCursorState,
) -> Result<bool, String> {
    RemoteFeishuGatewayEventSourceWriterRuntime::with_event_source_lock(|| {
        let event_source_lines = ShardedJsonlStore::new()
            .read_repaired_lines(event_source_path)
            .map_err(|_| {
                axon_domain::AxonError::runtime_failure("Feishu gateway 事件源读取失败")
            })?;
        if event_source_lines.is_empty() || event_source_lines.len() as u64 != cursor.acked_offset {
            return Ok(false);
        }
        cursor.acked_offset = 0;
        write_cursor_state(cursor_path, cursor).map_err(axon_domain::AxonError::runtime_failure)?;
        ShardedJsonlStore::new()
            .remove_all(event_source_path)
            .map_err(|_| axon_domain::AxonError::runtime_failure("Feishu gateway 事件源清空失败"))
    })
    .map_err(|error| format!("{error:?}"))
}

pub(super) fn resolve_cursor_file(command: &RemoteGatewayCommand, event_file: &Path) -> PathBuf {
    command
        .cursor_file
        .clone()
        .unwrap_or_else(|| event_file.with_extension("cursor.json"))
}

pub(super) fn read_cursor_state(path: &Path) -> Result<RemoteFeishuGatewayCursorState, String> {
    if !path.exists() {
        return Ok(RemoteFeishuGatewayCursorState::default());
    }
    let content = fs::read_to_string(path)
        .map_err(|error| format!("读取 Feishu gateway cursor 失败: {error}"))?;
    serde_json::from_str::<RemoteFeishuGatewayCursorState>(&content)
        .map_err(|error| format!("Feishu gateway cursor JSON 无效: {error}"))
}

pub(super) fn write_cursor_state(
    path: &Path,
    cursor: &RemoteFeishuGatewayCursorState,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 Feishu gateway cursor 目录失败: {error}"))?;
    }
    let content = serde_json::to_vec(cursor)
        .map_err(|error| format!("序列化 Feishu gateway cursor 失败: {error}"))?;
    let temporary_path = path.with_extension("tmp");
    let mut temporary = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary_path)
        .map_err(|error| format!("打开 Feishu gateway cursor 临时文件失败: {error}"))?;
    temporary
        .write_all(&content)
        .and_then(|()| temporary.sync_all())
        .map_err(|error| format!("写入 Feishu gateway cursor 临时文件失败: {error}"))?;
    drop(temporary);
    fs::rename(&temporary_path, path)
        .map_err(|error| format!("替换 Feishu gateway cursor 失败: {error}"))
}

fn default_event_kind() -> String {
    "message".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_jsonl::ShardedJsonlStore;

    #[test]
    fn cursor_reads_only_complete_events_after_line_offset() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("events.jsonl");
        let first = "{\"kind\":\"message\",\"requestId\":\"req-1\",\"chatId\":\"oc_1\",\"messageId\":\"om_1\",\"senderOpenId\":\"ou_1\",\"text\":\"第一条\"}\n";
        let partial_second = "{\"kind\":\"message\",\"requestId\":\"req-2\",\"chatId\":\"oc_1\",\"messageId\":\"om_2\",\"senderOpenId\":\"ou_1\",\"text\":\"第二条\"}";
        fs::write(
            ShardedJsonlStore::physical_shard_path(&path, 0),
            format!("{first}{partial_second}"),
        )
        .expect("写入事件源");

        let (first_events, first_offset) = read_gateway_input_events_after_cursor(
            &path,
            0,
            REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS,
        )
        .expect("读取完整事件");

        assert_eq!(first_events.len(), 1);
        assert_eq!(first_events[0].request_id, "req-1");
        assert_eq!(first_offset, 1_u64);

        fs::write(
            ShardedJsonlStore::physical_shard_path(&path, 0),
            format!(
                "{first}{{\"kind\":\"message\",\"requestId\":\"req-2\",\"chatId\":\"oc_1\",\"messageId\":\"om_2\",\"senderOpenId\":\"ou_1\",\"text\":\"第二条\"}}\n"
            ),
        )
        .expect("完成事件写入");
        let (second_events, second_offset) = read_gateway_input_events_after_cursor(
            &path,
            first_offset,
            REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS,
        )
        .expect("读取后续事件");

        assert_eq!(second_events.len(), 1);
        assert_eq!(second_events[0].request_id, "req-2");
        assert_eq!(second_offset, 2_u64);
    }

    #[test]
    fn cursor_state_replaces_previous_value_without_leaving_temp_file() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("gateway.cursor.json");
        let cursor = RemoteFeishuGatewayCursorState { acked_offset: 128 };

        write_cursor_state(&path, &cursor).expect("写入 cursor");

        assert_eq!(read_cursor_state(&path).expect("读取 cursor"), cursor);
        assert!(!path.with_extension("tmp").exists());
    }

    #[test]
    fn cursor_state_rejects_obsolete_line_cursor() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("gateway.cursor.json");
        fs::write(&path, r#"{"ackedLine":1}"#).expect("写入旧 cursor");

        let error = read_cursor_state(&path).expect_err("旧行号 cursor 必须被拒绝");

        assert!(error.starts_with("Feishu gateway cursor JSON 无效"));
    }

    #[test]
    fn cursor_reads_at_most_the_fixed_event_batch() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("events.jsonl");
        let event = "{\"kind\":\"message\",\"requestId\":\"req\",\"chatId\":\"oc\",\"messageId\":\"om\",\"senderOpenId\":\"ou\",\"text\":\"消息\"}\n";
        fs::write(
            ShardedJsonlStore::physical_shard_path(&path, 0),
            event.repeat(REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS + 1),
        )
        .expect("写入事件源");

        let (events, offset) = read_gateway_input_events_after_cursor(
            &path,
            0,
            REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS,
        )
        .expect("读取固定批次");

        assert_eq!(
            events.len(),
            REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS
        );
        assert_eq!(
            offset,
            u64::try_from(REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS).unwrap_or(u64::MAX)
        );
    }

    #[test]
    fn cursor_skips_malformed_completed_jsonl_record_and_continues() {
        let directory = axon_test_support::test_temp_dir();
        let path = directory.path().join("events.jsonl");
        let malformed = b"{bad json}\n";
        let valid = b"{\"kind\":\"message\",\"requestId\":\"req-valid\",\"chatId\":\"oc\",\"messageId\":\"om\",\"senderOpenId\":\"ou\",\"text\":\"message\"}\n";
        let invalid_utf8 = b"\xff\n";
        let contents = [
            malformed.as_slice(),
            valid.as_slice(),
            invalid_utf8.as_slice(),
        ]
        .concat();
        fs::write(ShardedJsonlStore::physical_shard_path(&path, 0), &contents).expect("写入事件源");

        let (events, offset) = read_gateway_input_events_after_cursor(
            &path,
            0,
            REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS,
        )
        .expect("坏记录不应阻塞读取");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].request_id, "req-valid");
        assert_eq!(offset, 3_u64);
    }

    #[test]
    fn confirmed_source_is_cleared_only_when_no_new_event_was_appended() {
        let directory = axon_test_support::test_temp_dir();
        let event_path = directory.path().join("events.jsonl");
        let cursor_path = directory.path().join("events.cursor.json");
        fs::write(
            ShardedJsonlStore::physical_shard_path(&event_path, 0),
            b"{}\n",
        )
        .expect("写入事件源");
        let mut cursor = RemoteFeishuGatewayCursorState { acked_offset: 1 };

        assert!(
            reset_gateway_event_source_if_fully_acknowledged(
                &event_path,
                &cursor_path,
                &mut cursor,
            )
            .expect("清空已确认事件源")
        );
        assert_eq!(cursor.acked_offset, 0);
        assert!(
            ShardedJsonlStore::new()
                .shard_paths(&event_path)
                .expect("读取事件源 shard")
                .is_empty()
        );

        fs::write(
            ShardedJsonlStore::physical_shard_path(&event_path, 0),
            b"{}\n",
        )
        .expect("写入新事件");
        assert!(
            !reset_gateway_event_source_if_fully_acknowledged(
                &event_path,
                &cursor_path,
                &mut cursor,
            )
            .expect("空 cursor 不应清空新事件")
        );
    }
}
