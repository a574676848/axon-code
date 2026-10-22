//! Feishu gateway append-only 入站事件源 writer。
//!
//! 本模块只负责把 SDK/外部 adapter 已归一化的入站事件追加写入 JSONL 事件源，
//! daemon shell 再按 cursor/ack 语义读取，不在这里执行治理或远控主链。

#[cfg(test)]
use std::fs;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use axon_domain::{AxonError, AxonResult};
use axon_jsonl::ShardedJsonlStore;
use serde::Serialize;

use crate::{
    RemoteFeishuCardActionEvent, RemoteFeishuGatewayInboundEvent, RemoteFeishuIncomingMessage,
    RemoteFeishuMediaRef, RemoteFeishuReplyContext,
};

/// Feishu gateway 默认 JSONL 事件源文件名。
pub const REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_FILE: &str = "remote_feishu_gateway_events.jsonl";
/// Feishu gateway 事件源最大磁盘占用。
pub const REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_BYTES: u64 = 16 * 1024 * 1024;
/// Feishu gateway 单条事件的最大 JSONL 字节数。
pub const REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_RECORD_BYTES: usize = 64 * 1024;
/// Feishu gateway 单轮最多读取的事件数。
pub const REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_READ_EVENTS: usize = 8;

static EVENT_SOURCE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
/// 事件源写入使用的带总容量预算的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// Feishu gateway append-only writer 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayEventSourceAppendInput {
    /// 事件源路径。
    pub event_source_path: PathBuf,
    /// 入站事件。
    pub event: RemoteFeishuGatewayInboundEvent,
    /// 飞书消息源创建时间；非消息事件为空。
    pub source_created_at_ms: Option<u64>,
    /// 入队时间。
    pub enqueued_at_ms: u64,
}

/// Feishu gateway append-only writer 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayEventSourceAppendReport {
    /// 事件源路径。
    pub event_source_path: PathBuf,
    /// 本次写入的 JSONL 字节数。
    pub appended_bytes: usize,
    /// 入站 request_id。
    pub request_id: String,
    /// 入站事件类型。
    pub kind: &'static str,
}

/// 尚未完成消息归属查询的 reaction 原始事件追加输入。
///
/// 原始 payload 仅保存在本地事件源，worker 读取后再做 HTTP enrichment；
/// 回调线程不依赖外部网络即可完成 durable append。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewayRawReactionAppendInput {
    /// 事件源路径。
    pub event_source_path: PathBuf,
    /// reaction 请求 ID。
    pub request_id: String,
    /// 被 reaction 的消息 ID。
    pub message_id: String,
    /// payload 中可直接取得的 chat ID。
    pub chat_id: Option<String>,
    /// reaction 操作者 open ID。
    pub sender_open_id: Option<String>,
    /// 原始 reaction JSON。
    pub payload_json: String,
    /// 入队时间。
    pub enqueued_at_ms: u64,
}

/// Feishu gateway append-only 事件源 writer。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuGatewayEventSourceWriterRuntime;

impl RemoteFeishuGatewayEventSourceWriterRuntime {
    /// 解析默认事件源路径。
    #[must_use]
    pub fn resolve_event_source_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_FILE)
    }

    /// 在事件源写入锁内执行操作。
    ///
    /// 该锁覆盖容量检查、追加和已确认事件源的截断。正式 gateway 由 app lock
    /// 保证单进程运行；这里再处理同一进程内多个 WebSocket callback 的并发。
    pub fn with_event_source_lock<T>(operation: impl FnOnce() -> AxonResult<T>) -> AxonResult<T> {
        let lock = EVENT_SOURCE_LOCK.get_or_init(|| Mutex::new(()));
        let _guard = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        operation()
    }

    /// 追加写入单条入站事件。
    ///
    /// # Errors
    ///
    /// 当事件源路径、事件内容、写入时间或 JSONL 写入失败时返回错误。
    pub fn append_event(
        input: RemoteFeishuGatewayEventSourceAppendInput,
    ) -> AxonResult<RemoteFeishuGatewayEventSourceAppendReport> {
        if input.enqueued_at_ms == 0 {
            return Err(AxonError::runtime_failure(
                "Feishu gateway 事件源写入时间无效",
            ));
        }
        let record = event_record(
            &input.event,
            input.source_created_at_ms,
            input.enqueued_at_ms,
        )?;
        let line = serde_json::to_vec(&record)
            .map_err(|_| AxonError::runtime_failure("Feishu gateway 事件源记录序列化失败"))?;
        if line.len() > REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_RECORD_BYTES {
            return Err(AxonError::runtime_failure(
                "Feishu gateway 单条事件超过事件源预算",
            ));
        }
        Self::with_event_source_lock(|| {
            append_line_within_budget(&input.event_source_path, &line)?;
            Ok(RemoteFeishuGatewayEventSourceAppendReport {
                event_source_path: input.event_source_path,
                appended_bytes: line.len().saturating_add(1),
                request_id: record.request_id,
                kind: record.kind,
            })
        })
    }

    /// 追加尚未 enrichment 的 reaction 原始 payload。
    pub fn append_raw_reaction(
        input: RemoteFeishuGatewayRawReactionAppendInput,
    ) -> AxonResult<RemoteFeishuGatewayEventSourceAppendReport> {
        if input.enqueued_at_ms == 0
            || input.request_id.trim().is_empty()
            || input.message_id.trim().is_empty()
            || input.payload_json.trim().is_empty()
        {
            return Err(AxonError::runtime_failure(
                "Feishu reaction 原始事件源记录无效",
            ));
        }
        let record = RemoteFeishuGatewayEventSourceRecord {
            kind: "reactionPending",
            request_id: input.request_id,
            chat_id: input
                .chat_id
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| format!("reaction-pending:{}", input.message_id)),
            chat_type: None,
            message_id: Some(input.message_id),
            source_created_at_ms: None,
            sender_open_id: input.sender_open_id,
            operator_open_id: None,
            mentions: Vec::new(),
            text: Some("reaction:pending".to_owned()),
            media_refs: Vec::new(),
            message_kind: Some("text"),
            action_tag: None,
            action_value_json: None,
            reply_context: None,
            session_busy: false,
            enqueued_at_ms: input.enqueued_at_ms,
            raw_payload_json: Some(input.payload_json),
        };
        let line = serde_json::to_vec(&record)
            .map_err(|_| AxonError::runtime_failure("Feishu reaction 原始事件序列化失败"))?;
        if line.len() > REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_RECORD_BYTES {
            return Err(AxonError::runtime_failure(
                "Feishu reaction 原始事件超过事件源预算",
            ));
        }
        Self::with_event_source_lock(|| {
            append_line_within_budget(&input.event_source_path, &line)?;
            Ok(RemoteFeishuGatewayEventSourceAppendReport {
                event_source_path: input.event_source_path,
                appended_bytes: line.len().saturating_add(1),
                request_id: record.request_id,
                kind: record.kind,
            })
        })
    }
}

fn append_line_within_budget(path: &Path, line: &[u8]) -> AxonResult<()> {
    SHARD_STORE
        .append_bounded_line(path, line, REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_BYTES)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::ResourceBusy {
                AxonError::runtime_failure("Feishu gateway 事件源积压已达到容量上限")
            } else {
                AxonError::runtime_failure("Feishu gateway 事件源写入失败")
            }
        })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFeishuGatewayEventSourceRecord {
    kind: &'static str,
    request_id: String,
    chat_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    chat_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_created_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sender_open_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operator_open_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    mentions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    media_refs: Vec<RemoteFeishuMediaRefRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_value_json: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_context: Option<RemoteFeishuGatewayEventSourceReplyContext>,
    session_busy: bool,
    enqueued_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    raw_payload_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFeishuGatewayEventSourceReplyContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thread_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteFeishuMediaRefRecord {
    kind: &'static str,
    key: String,
    file_name: Option<String>,
}

fn event_record(
    event: &RemoteFeishuGatewayInboundEvent,
    source_created_at_ms: Option<u64>,
    enqueued_at_ms: u64,
) -> AxonResult<RemoteFeishuGatewayEventSourceRecord> {
    match event {
        RemoteFeishuGatewayInboundEvent::Message(message) => {
            message_record(message, source_created_at_ms, enqueued_at_ms)
        }
        RemoteFeishuGatewayInboundEvent::CardAction(card_action) => {
            card_action_record(card_action, enqueued_at_ms)
        }
    }
}

fn message_record(
    message: &RemoteFeishuIncomingMessage,
    source_created_at_ms: Option<u64>,
    enqueued_at_ms: u64,
) -> AxonResult<RemoteFeishuGatewayEventSourceRecord> {
    if message.request_id.trim().is_empty()
        || message.chat_id.trim().is_empty()
        || message.message_id.trim().is_empty()
        || message.sender_open_id.trim().is_empty()
        || message.text.trim().is_empty()
    {
        return Err(AxonError::runtime_failure(
            "Feishu gateway message 事件源记录无效",
        ));
    }
    Ok(RemoteFeishuGatewayEventSourceRecord {
        kind: "message",
        request_id: message.request_id.clone(),
        chat_id: message.chat_id.clone(),
        chat_type: Some(message.chat_type.as_str()),
        message_id: Some(message.message_id.clone()),
        source_created_at_ms,
        sender_open_id: Some(message.sender_open_id.clone()),
        operator_open_id: None,
        mentions: message.mentions.clone(),
        text: Some(message.text.clone()),
        media_refs: media_ref_records(&message.media_refs),
        message_kind: Some(message.message_kind.as_str()),
        action_tag: None,
        action_value_json: None,
        reply_context: reply_context_record(message.reply_context.as_ref()),
        session_busy: message.session_busy,
        enqueued_at_ms,
        raw_payload_json: None,
    })
}

fn card_action_record(
    card_action: &RemoteFeishuCardActionEvent,
    enqueued_at_ms: u64,
) -> AxonResult<RemoteFeishuGatewayEventSourceRecord> {
    if card_action.request_id.trim().is_empty()
        || card_action.chat_id.trim().is_empty()
        || card_action.operator_open_id.trim().is_empty()
        || card_action.action_tag.trim().is_empty()
    {
        return Err(AxonError::runtime_failure(
            "Feishu gateway cardAction 事件源记录无效",
        ));
    }
    Ok(RemoteFeishuGatewayEventSourceRecord {
        kind: "cardAction",
        request_id: card_action.request_id.clone(),
        chat_id: card_action.chat_id.clone(),
        chat_type: None,
        message_id: card_action.message_id.clone(),
        source_created_at_ms: None,
        sender_open_id: None,
        operator_open_id: Some(card_action.operator_open_id.clone()),
        mentions: Vec::new(),
        text: None,
        media_refs: Vec::new(),
        message_kind: None,
        action_tag: Some(card_action.action_tag.clone()),
        action_value_json: Some(card_action.action_value_json.clone()),
        reply_context: reply_context_record(card_action.reply_context.as_ref()),
        session_busy: card_action.session_busy,
        enqueued_at_ms,
        raw_payload_json: None,
    })
}

fn reply_context_record(
    reply_context: Option<&RemoteFeishuReplyContext>,
) -> Option<RemoteFeishuGatewayEventSourceReplyContext> {
    reply_context.map(|context| RemoteFeishuGatewayEventSourceReplyContext {
        reply_to_message_id: context.reply_to_message_id.clone(),
        reply_to_text: context.reply_to_text.clone(),
        thread_id: context.thread_id.clone(),
    })
}

fn media_ref_records(refs: &[RemoteFeishuMediaRef]) -> Vec<RemoteFeishuMediaRefRecord> {
    refs.iter()
        .map(|media| RemoteFeishuMediaRefRecord {
            kind: media.kind.as_str(),
            key: media.key.clone(),
            file_name: media.file_name.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use axon_domain::SessionId;

    use super::*;

    fn read_source(path: &Path) -> String {
        let lines = SHARD_STORE.read_lines(path).expect("读取事件源 shard");
        let mut content = Vec::new();
        for line in lines {
            content.extend_from_slice(&line);
            content.push(b'\n');
        }
        String::from_utf8(content).expect("事件源 UTF-8")
    }

    #[test]
    fn gateway_event_source_writer_appends_jsonl_message_records() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-gateway-events-{}.jsonl",
            unique_suffix()
        ));

        RemoteFeishuGatewayEventSourceWriterRuntime::append_event(
            RemoteFeishuGatewayEventSourceAppendInput {
                event_source_path: path.clone(),
                event: RemoteFeishuGatewayInboundEvent::Message(message("req-1")),
                source_created_at_ms: Some(90),
                enqueued_at_ms: 100,
            },
        )
        .expect("append first");
        RemoteFeishuGatewayEventSourceWriterRuntime::append_event(
            RemoteFeishuGatewayEventSourceAppendInput {
                event_source_path: path.clone(),
                event: RemoteFeishuGatewayInboundEvent::Message(message("req-2")),
                source_created_at_ms: None,
                enqueued_at_ms: 101,
            },
        )
        .expect("append second");
        let content = read_source(&path);

        assert_eq!(content.lines().count(), 2);
        assert!(content.contains("\"kind\":\"message\""));
        assert!(content.contains("\"chatType\":\"p2p\""));
        assert!(content.contains("\"messageKind\":\"text\""));
        assert!(content.contains("\"requestId\":\"req-1\""));
        assert!(content.contains("\"sourceCreatedAtMs\":90"));
        let _ = SHARD_STORE.remove_all(path);
    }

    #[test]
    fn gateway_event_source_writer_appends_card_action_records() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-gateway-card-events-{}.jsonl",
            unique_suffix()
        ));

        let report = RemoteFeishuGatewayEventSourceWriterRuntime::append_event(
            RemoteFeishuGatewayEventSourceAppendInput {
                event_source_path: path.clone(),
                event: RemoteFeishuGatewayInboundEvent::CardAction(card_action()),
                source_created_at_ms: None,
                enqueued_at_ms: 100,
            },
        )
        .expect("append card action");
        let content = read_source(&path);

        assert_eq!(report.kind, "cardAction");
        assert!(content.contains("\"actionTag\":\"approve\""));
        let _ = SHARD_STORE.remove_all(path);
    }

    #[test]
    fn gateway_event_source_writer_appends_raw_reaction_before_enrichment() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-gateway-reaction-pending-{}.jsonl",
            unique_suffix()
        ));
        let report = RemoteFeishuGatewayEventSourceWriterRuntime::append_raw_reaction(
            RemoteFeishuGatewayRawReactionAppendInput {
                event_source_path: path.clone(),
                request_id: "evt-reaction".to_owned(),
                message_id: "om-reaction".to_owned(),
                chat_id: None,
                sender_open_id: Some("ou-reactor".to_owned()),
                payload_json: r#"{"event":{"message_id":"om-reaction"}}"#.to_owned(),
                enqueued_at_ms: 100,
            },
        )
        .expect("追加原始 reaction");

        assert_eq!(report.kind, "reactionPending");
        let content = read_source(&path);
        assert!(content.contains("rawPayloadJson"));
        assert!(content.contains("reaction-pending:om-reaction"));
        let _ = SHARD_STORE.remove_all(path);
    }

    #[test]
    fn gateway_event_source_writer_rejects_records_over_the_fixed_budget() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-gateway-oversized-event-{}.jsonl",
            unique_suffix()
        ));
        let mut oversized = message("req-oversized");
        oversized.text = "x".repeat(REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_RECORD_BYTES);

        let error = RemoteFeishuGatewayEventSourceWriterRuntime::append_event(
            RemoteFeishuGatewayEventSourceAppendInput {
                event_source_path: path.clone(),
                event: RemoteFeishuGatewayInboundEvent::Message(oversized),
                source_created_at_ms: None,
                enqueued_at_ms: 100,
            },
        )
        .expect_err("超出预算的事件必须被拒绝");

        assert!(format!("{error:?}").contains("超过事件源预算"));
        assert!(!path.exists());
    }

    #[test]
    fn gateway_event_source_writer_rejects_append_when_source_budget_is_full() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-gateway-full-event-source-{}.jsonl",
            unique_suffix()
        ));
        fs::create_dir_all(path.parent().expect("parent")).expect("parent");
        let mut full_line = vec![b'x'; REMOTE_FEISHU_GATEWAY_EVENT_SOURCE_MAX_BYTES as usize - 1];
        full_line.push(b'\n');
        fs::write(ShardedJsonlStore::physical_shard_path(&path, 0), full_line)
            .expect("占用事件源预算");

        let error = RemoteFeishuGatewayEventSourceWriterRuntime::append_event(
            RemoteFeishuGatewayEventSourceAppendInput {
                event_source_path: path.clone(),
                event: RemoteFeishuGatewayInboundEvent::Message(message("req-full")),
                source_created_at_ms: None,
                enqueued_at_ms: 100,
            },
        )
        .expect_err("满容量事件源必须拒绝追加");

        assert!(format!("{error:?}").contains("积压已达到容量上限"));
        let _ = SHARD_STORE.remove_all(path);
    }

    fn message(request_id: &str) -> RemoteFeishuIncomingMessage {
        RemoteFeishuIncomingMessage {
            session_id: SessionId(18),
            client_id: "feishu-sdk".to_owned(),
            request_id: request_id.to_owned(),
            message_id: format!("om_{request_id}"),
            chat_id: "oc_gateway".to_owned(),
            chat_type: crate::RemoteFeishuChatType::P2p,
            sender_open_id: "ou_simulated".to_owned(),
            mentions: vec!["ou_bot".to_owned()],
            text: "继续推进".to_owned(),
            media_refs: Vec::new(),
            message_kind: crate::RemoteFeishuIncomingMessageKind::Text,
            reply_context: None,
            session_busy: false,
        }
    }

    fn card_action() -> RemoteFeishuCardActionEvent {
        RemoteFeishuCardActionEvent {
            session_id: SessionId(18),
            client_id: "feishu-sdk".to_owned(),
            request_id: "card-req-1".to_owned(),
            message_id: Some("om_card_req_1".to_owned()),
            chat_id: "oc_gateway".to_owned(),
            operator_open_id: "ou_simulated".to_owned(),
            action_tag: "approve".to_owned(),
            action_value_json: "{}".to_owned(),
            reply_context: None,
            session_busy: true,
        }
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    }
}
