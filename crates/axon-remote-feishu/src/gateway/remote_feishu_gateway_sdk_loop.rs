//! Feishu SDK loop 到独立 gateway 事件源的 adapter。
//!
//! 真实 Feishu SDK loop 收到事件后，只在这里完成入站事件校验与 gateway 事件源写入，
//! 不直接触发治理、session 或 remote transport 主链。

use std::path::PathBuf;

use axon_domain::{AxonResult, SessionId};

use crate::{
    RemoteFeishuBinaryHttpResponse, RemoteFeishuCardActionEvent, RemoteFeishuChatType,
    RemoteFeishuGatewayEventSourceAppendInput, RemoteFeishuGatewayEventSourceAppendReport,
    RemoteFeishuGatewayEventSourceWriterRuntime, RemoteFeishuGatewayInboundEvent,
    RemoteFeishuHttpRequestPlan, RemoteFeishuInboundMediaDownloadInput,
    RemoteFeishuIncomingMessage, RemoteFeishuIncomingMessageKind, RemoteFeishuMediaFlowRuntime,
    RemoteFeishuMediaRef, RemoteFeishuOapiExecutorRuntime, RemoteFeishuReplyContext,
    RemoteFeishuWebSocketIngressRuntime,
};

/// Feishu SDK loop 事件已归一化原因。
pub const REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_NORMALIZED: &str =
    "remote_feishu_gateway_sdk_loop:normalized";
/// Feishu SDK loop 事件已写入事件源原因。
pub const REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_EVENT_SOURCE_APPENDED: &str =
    "remote_feishu_gateway_sdk_loop:event_source_appended";

/// Feishu SDK loop adapter 写入输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopAppendInput {
    /// gateway JSONL 事件源路径。
    pub event_source_path: PathBuf,
    /// 飞书消息源创建时间；非消息事件为空。
    pub source_created_at_ms: Option<u64>,
    /// 入队时间。
    pub enqueued_at_ms: u64,
}

/// Feishu SDK loop adapter 写入报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopAppendReport {
    /// append-only writer 报告。
    pub append: RemoteFeishuGatewayEventSourceAppendReport,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Feishu SDK loop runner shell 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopRunnerInput {
    /// gateway JSONL 事件源路径。
    pub event_source_path: PathBuf,
    /// 目标会话。
    pub session_id: SessionId,
    /// SDK loop client 标识。
    pub client_id: String,
    /// 入队时间。
    pub enqueued_at_ms: u64,
    /// SDK loop 已收到的事件流。
    pub events: Vec<RemoteFeishuGatewaySdkLoopEvent>,
    /// 可选媒体下载上下文；为空时仅写入媒体引用，不执行 OpenAPI 下载。
    pub media_download: Option<RemoteFeishuGatewaySdkLoopMediaDownloadContext>,
}

/// Feishu SDK loop 媒体下载上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopMediaDownloadContext {
    /// tenant_access_token。
    pub tenant_access_token: String,
    /// OpenAPI 基址。
    pub api_base_url: Option<String>,
    /// 本地媒体缓存根目录。
    pub cache_root: PathBuf,
}

/// Feishu SDK loop runner shell 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFeishuGatewaySdkLoopEvent {
    /// 文本消息回调。
    Message(RemoteFeishuGatewaySdkLoopMessageEvent),
    /// 卡片动作回调。
    CardAction(RemoteFeishuGatewaySdkLoopCardActionEvent),
}

/// Feishu SDK loop 文本消息回调字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopMessageEvent {
    /// 请求 ID。
    pub request_id: String,
    /// Feishu message_id。
    pub message_id: String,
    /// 飞书消息源创建时间。
    pub source_created_at_ms: Option<u64>,
    /// Feishu chat 标识。
    pub chat_id: String,
    /// Feishu 会话类型。
    pub chat_type: RemoteFeishuChatType,
    /// 发送者 open_id。
    pub sender_open_id: String,
    /// 被 @ 的 open_id 列表。
    pub mentions: Vec<String>,
    /// 消息文本。
    pub text: String,
    /// 入站媒体引用。
    pub media_refs: Vec<RemoteFeishuMediaRef>,
    /// Axon 入站文本分类。
    pub message_kind: RemoteFeishuIncomingMessageKind,
    /// 可选 reply/thread 上下文。
    pub reply_context: Option<RemoteFeishuReplyContext>,
    /// 当前会话是否忙碌。
    pub session_busy: bool,
}

/// Feishu SDK loop 卡片动作回调字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopCardActionEvent {
    /// 请求或 token ID。
    pub request_id: String,
    /// 被点击卡片自身的消息 ID（`context.open_message_id`）；缺失时为空。
    pub message_id: Option<String>,
    /// Feishu chat 标识。
    pub chat_id: String,
    /// 操作人 open_id。
    pub operator_open_id: String,
    /// 卡片动作标签。
    pub action_tag: String,
    /// 卡片动作值 JSON。
    pub action_value_json: String,
    /// 可选 reply/thread 上下文。
    pub reply_context: Option<RemoteFeishuReplyContext>,
    /// 当前会话是否忙碌。
    pub session_busy: bool,
}

/// Feishu SDK loop runner shell 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuGatewaySdkLoopRunnerReport {
    /// 输入事件数。
    pub input_events: usize,
    /// 已追加事件数。
    pub appended_events: usize,
    /// append-only writer 报告。
    pub appended: Vec<RemoteFeishuGatewayEventSourceAppendReport>,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Feishu SDK loop 到 gateway 事件源的 adapter runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuGatewaySdkLoopAdapterRuntime;

impl RemoteFeishuGatewaySdkLoopAdapterRuntime {
    /// 运行 SDK loop 事件流 shell。
    ///
    /// # Errors
    ///
    /// 当 SDK loop 公共上下文、事件结构或事件源写入失败时返回错误。
    pub fn run_events(
        input: RemoteFeishuGatewaySdkLoopRunnerInput,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopRunnerReport> {
        Self::run_events_with_interceptor(input, |_| Ok(false))
    }

    /// 运行 SDK loop 事件流，并允许上层在写入事件源前接管单个事件。
    pub fn run_events_with_interceptor<F>(
        input: RemoteFeishuGatewaySdkLoopRunnerInput,
        mut interceptor: F,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopRunnerReport>
    where
        F: FnMut(&RemoteFeishuGatewaySdkLoopEvent) -> AxonResult<bool>,
    {
        validate_runner_input(&input)?;
        let RemoteFeishuGatewaySdkLoopRunnerInput {
            event_source_path,
            session_id,
            client_id,
            enqueued_at_ms,
            events,
            media_download,
        } = input;
        let input_events = events.len();
        let mut appended = Vec::with_capacity(input_events);
        for event in events {
            if interceptor(&event)? {
                continue;
            }
            let source_created_at_ms = match &event {
                RemoteFeishuGatewaySdkLoopEvent::Message(message) => message.source_created_at_ms,
                RemoteFeishuGatewaySdkLoopEvent::CardAction(_) => None,
            };
            let append = RemoteFeishuGatewaySdkLoopAppendInput {
                event_source_path: event_source_path.clone(),
                source_created_at_ms,
                enqueued_at_ms,
            };
            let report = match event {
                RemoteFeishuGatewaySdkLoopEvent::Message(message) => match &media_download {
                    Some(context) => Self::handle_message_with_media_download(
                        &message.into_incoming(session_id, &client_id),
                        append,
                        context.tenant_access_token.clone(),
                        context.api_base_url.clone(),
                        context.cache_root.clone(),
                    )?,
                    None => Self::handle_message(
                        &message.into_incoming(session_id, &client_id),
                        append,
                    )?,
                },
                RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action) => {
                    Self::handle_card_action(
                        &card_action.into_card_action(session_id, &client_id),
                        append,
                    )?
                }
            };
            appended.push(report.append);
        }
        Ok(RemoteFeishuGatewaySdkLoopRunnerReport {
            input_events,
            appended_events: appended.len(),
            appended,
            reasons: vec![
                REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_NORMALIZED.to_owned(),
                REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_EVENT_SOURCE_APPENDED.to_owned(),
            ],
        })
    }

    /// 处理 SDK loop 收到的文本消息事件。
    ///
    /// # Errors
    ///
    /// 当消息事件无效或事件源写入失败时返回错误。
    pub fn handle_message(
        event: &RemoteFeishuIncomingMessage,
        input: RemoteFeishuGatewaySdkLoopAppendInput,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopAppendReport> {
        RemoteFeishuWebSocketIngressRuntime::normalize_message(event)?;
        append_gateway_event(
            RemoteFeishuGatewayInboundEvent::Message(event.clone()),
            input,
        )
    }

    /// 处理 SDK loop 收到的消息事件，先下载媒体并把本地路径注入事件文本。
    ///
    /// # Errors
    ///
    /// 当消息事件、媒体下载或事件源写入失败时返回错误。
    pub fn handle_message_with_media_download(
        event: &RemoteFeishuIncomingMessage,
        input: RemoteFeishuGatewaySdkLoopAppendInput,
        tenant_access_token: String,
        api_base_url: Option<String>,
        cache_root: PathBuf,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopAppendReport> {
        Self::handle_message_with_media_download_and_http(
            event,
            input,
            tenant_access_token,
            api_base_url,
            cache_root,
            RemoteFeishuOapiExecutorRuntime::execute_binary,
        )
    }

    /// 处理 SDK loop 消息事件，并允许测试注入媒体下载执行器。
    ///
    /// # Errors
    ///
    /// 当消息事件、媒体下载或事件源写入失败时返回错误。
    pub fn handle_message_with_media_download_and_http<F>(
        event: &RemoteFeishuIncomingMessage,
        input: RemoteFeishuGatewaySdkLoopAppendInput,
        tenant_access_token: String,
        api_base_url: Option<String>,
        cache_root: PathBuf,
        mut http: F,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopAppendReport>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuBinaryHttpResponse>,
    {
        let message = if event.media_refs.is_empty() {
            event.clone()
        } else {
            RemoteFeishuMediaFlowRuntime::download_inbound_media_with_http(
                RemoteFeishuInboundMediaDownloadInput {
                    message: event.clone(),
                    tenant_access_token,
                    api_base_url,
                    cache_root,
                },
                &mut http,
            )?
            .message
        };
        Self::handle_message(&message, input)
    }

    /// 处理 SDK loop 收到的卡片动作事件。
    ///
    /// # Errors
    ///
    /// 当卡片动作事件无效或事件源写入失败时返回错误。
    pub fn handle_card_action(
        event: &RemoteFeishuCardActionEvent,
        input: RemoteFeishuGatewaySdkLoopAppendInput,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopAppendReport> {
        RemoteFeishuWebSocketIngressRuntime::normalize_card_action(event)?;
        append_gateway_event(
            RemoteFeishuGatewayInboundEvent::CardAction(event.clone()),
            input,
        )
    }
}

impl RemoteFeishuGatewaySdkLoopMessageEvent {
    fn into_incoming(self, session_id: SessionId, client_id: &str) -> RemoteFeishuIncomingMessage {
        RemoteFeishuIncomingMessage {
            session_id,
            client_id: client_id.to_owned(),
            request_id: self.request_id,
            message_id: self.message_id,
            chat_id: self.chat_id,
            chat_type: self.chat_type,
            sender_open_id: self.sender_open_id,
            mentions: self.mentions,
            text: self.text,
            media_refs: self.media_refs,
            message_kind: self.message_kind,
            reply_context: self.reply_context,
            session_busy: self.session_busy,
        }
    }
}

impl RemoteFeishuGatewaySdkLoopCardActionEvent {
    fn into_card_action(
        self,
        session_id: SessionId,
        client_id: &str,
    ) -> RemoteFeishuCardActionEvent {
        RemoteFeishuCardActionEvent {
            session_id,
            client_id: client_id.to_owned(),
            request_id: self.request_id,
            message_id: self.message_id,
            chat_id: self.chat_id,
            operator_open_id: self.operator_open_id,
            action_tag: self.action_tag,
            action_value_json: self.action_value_json,
            reply_context: self.reply_context,
            session_busy: self.session_busy,
        }
    }
}

fn validate_runner_input(input: &RemoteFeishuGatewaySdkLoopRunnerInput) -> AxonResult<()> {
    if input.session_id.0 == 0 || input.client_id.trim().is_empty() {
        return Err(axon_domain::AxonError::runtime_failure(
            "Feishu SDK loop runner 公共上下文无效",
        ));
    }
    Ok(())
}

fn append_gateway_event(
    event: RemoteFeishuGatewayInboundEvent,
    input: RemoteFeishuGatewaySdkLoopAppendInput,
) -> AxonResult<RemoteFeishuGatewaySdkLoopAppendReport> {
    let append = RemoteFeishuGatewayEventSourceWriterRuntime::append_event(
        RemoteFeishuGatewayEventSourceAppendInput {
            event_source_path: input.event_source_path,
            event,
            source_created_at_ms: input.source_created_at_ms,
            enqueued_at_ms: input.enqueued_at_ms,
        },
    )?;
    Ok(RemoteFeishuGatewaySdkLoopAppendReport {
        append,
        reasons: vec![
            REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_NORMALIZED.to_owned(),
            REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_EVENT_SOURCE_APPENDED.to_owned(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use axon_domain::SessionId;
    use axon_jsonl::ShardedJsonlStore;

    use super::*;

    fn read_source(path: &Path) -> String {
        let lines = ShardedJsonlStore::new()
            .read_lines(path)
            .expect("读取事件源 shard");
        let mut content = Vec::new();
        for line in lines {
            content.extend_from_slice(&line);
            content.push(b'\n');
        }
        String::from_utf8(content).expect("事件源 UTF-8")
    }

    fn remove_source(path: PathBuf) {
        ShardedJsonlStore::new()
            .remove_all(path)
            .expect("删除 shard");
    }

    #[test]
    fn sdk_loop_adapter_writes_message_to_gateway_event_source() {
        let path = event_path("message");
        let report = RemoteFeishuGatewaySdkLoopAdapterRuntime::handle_message(
            &message("sdk-loop-req-1"),
            input(path.clone(), 1_000),
        )
        .expect("handle message");
        let content = read_source(&path);

        assert_eq!(report.append.kind, "message");
        assert!(
            report
                .reasons
                .iter()
                .any(|reason| reason == REMOTE_FEISHU_GATEWAY_SDK_LOOP_REASON_EVENT_SOURCE_APPENDED)
        );
        assert!(content.contains("\"kind\":\"message\""));
        assert!(content.contains("\"chatType\":\"p2p\""));
        assert!(content.contains("\"requestId\":\"sdk-loop-req-1\""));
        remove_source(path);
    }

    #[test]
    fn sdk_loop_adapter_writes_card_action_to_gateway_event_source() {
        let path = event_path("card");
        let report = RemoteFeishuGatewaySdkLoopAdapterRuntime::handle_card_action(
            &card_action(),
            input(path.clone(), 1_001),
        )
        .expect("handle card action");
        let content = read_source(&path);

        assert_eq!(report.append.kind, "cardAction");
        assert!(content.contains("\"kind\":\"cardAction\""));
        assert!(content.contains("\"actionTag\":\"button\""));
        remove_source(path);
    }

    #[test]
    fn sdk_loop_adapter_rejects_invalid_message_before_writing_event_source() {
        let path = event_path("invalid-message");
        let mut event = message("sdk-loop-invalid-req");
        event.text.clear();

        let result = RemoteFeishuGatewaySdkLoopAdapterRuntime::handle_message(
            &event,
            input(path.clone(), 1),
        );

        assert!(result.is_err());
        assert!(
            ShardedJsonlStore::new()
                .shard_paths(&path)
                .expect("检查事件源 shard")
                .is_empty()
        );
    }

    #[test]
    fn sdk_loop_adapter_downloads_media_before_writing_event_source() {
        let path = event_path("media-message");
        let root = path.parent().expect("temp parent").to_path_buf();
        let mut event = message("sdk-loop-media-req");
        event.media_refs = vec![crate::RemoteFeishuMediaRef {
            kind: crate::RemoteFeishuMediaKind::Image,
            key: "img_1".to_owned(),
            file_name: Some("screen.png".to_owned()),
        }];
        RemoteFeishuGatewaySdkLoopAdapterRuntime::handle_message_with_media_download_and_http(
            &event,
            input(path.clone(), 1_004),
            "tenant-token".to_owned(),
            Some("https://open.feishu.cn".to_owned()),
            root,
            |request| {
                assert!(request.url.contains("/resources/img_1"));
                Ok(RemoteFeishuBinaryHttpResponse {
                    status: 200,
                    body: b"png".to_vec(),
                    content_type: Some("image/png".to_owned()),
                })
            },
        )
        .expect("handle media message");
        let content = read_source(&path);
        let record: serde_json::Value = serde_json::from_str(content.trim()).expect("jsonl record");
        let text = record
            .get("text")
            .and_then(serde_json::Value::as_str)
            .expect("message text");
        let attachment_marker = "[Attachment: screen.png]\n";
        let cached_path = text
            .split_once(attachment_marker)
            .map(|(_, value)| PathBuf::from(value.trim()))
            .expect("attachment cache path");

        assert!(cached_path.is_file());
        assert!(!text.contains("local_path="));
        assert!(!text.contains("media_type="));
        let _ = fs::remove_file(cached_path);
        remove_source(path);
    }

    #[test]
    fn sdk_loop_runner_shell_writes_callback_event_stream() {
        let path = event_path("runner-stream");
        let report = RemoteFeishuGatewaySdkLoopAdapterRuntime::run_events(
            RemoteFeishuGatewaySdkLoopRunnerInput {
                event_source_path: path.clone(),
                session_id: SessionId(31),
                client_id: "feishu-sdk-loop".to_owned(),
                enqueued_at_ms: 1_002,
                events: vec![
                    RemoteFeishuGatewaySdkLoopEvent::Message(message_callback("runner-msg-1")),
                    RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action_callback()),
                ],
                media_download: None,
            },
        )
        .expect("run events");
        let content = read_source(&path);

        assert_eq!(report.input_events, 2);
        assert_eq!(report.appended_events, 2);
        assert_eq!(report.appended.len(), 2);
        assert_eq!(content.lines().count(), 2);
        assert!(content.contains("\"kind\":\"message\""));
        assert!(content.contains("\"kind\":\"cardAction\""));
        remove_source(path);
    }

    #[test]
    fn sdk_loop_runner_interceptor_consumes_event_without_duplicate_append() {
        let path = event_path("runner-intercepted");
        let report = RemoteFeishuGatewaySdkLoopAdapterRuntime::run_events_with_interceptor(
            RemoteFeishuGatewaySdkLoopRunnerInput {
                event_source_path: path.clone(),
                session_id: SessionId(31),
                client_id: "feishu-sdk-loop".to_owned(),
                enqueued_at_ms: 1_002,
                events: vec![
                    RemoteFeishuGatewaySdkLoopEvent::Message(message_callback("runner-msg-1")),
                    RemoteFeishuGatewaySdkLoopEvent::CardAction(card_action_callback()),
                ],
                media_download: None,
            },
            |event| Ok(matches!(event, RemoteFeishuGatewaySdkLoopEvent::Message(_))),
        )
        .expect("intercept events");
        let content = read_source(&path);

        assert_eq!(report.input_events, 2);
        assert_eq!(report.appended_events, 1);
        assert_eq!(content.lines().count(), 1);
        assert!(!content.contains("runner-msg-1"));
        assert!(content.contains("\"kind\":\"cardAction\""));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn sdk_loop_runner_shell_rejects_invalid_context_before_writing() {
        let path = event_path("runner-invalid-context");
        let result = RemoteFeishuGatewaySdkLoopAdapterRuntime::run_events(
            RemoteFeishuGatewaySdkLoopRunnerInput {
                event_source_path: path.clone(),
                session_id: SessionId(0),
                client_id: "feishu-sdk-loop".to_owned(),
                enqueued_at_ms: 1_003,
                events: vec![RemoteFeishuGatewaySdkLoopEvent::Message(message_callback(
                    "runner-invalid-context",
                ))],
                media_download: None,
            },
        );

        assert!(result.is_err());
        assert!(
            ShardedJsonlStore::new()
                .shard_paths(&path)
                .expect("检查事件源 shard")
                .is_empty()
        );
    }

    fn input(path: PathBuf, enqueued_at_ms: u64) -> RemoteFeishuGatewaySdkLoopAppendInput {
        RemoteFeishuGatewaySdkLoopAppendInput {
            event_source_path: path,
            source_created_at_ms: None,
            enqueued_at_ms,
        }
    }

    fn message(request_id: &str) -> RemoteFeishuIncomingMessage {
        RemoteFeishuIncomingMessage {
            session_id: SessionId(31),
            client_id: "feishu-sdk-loop".to_owned(),
            request_id: request_id.to_owned(),
            message_id: format!("om_{request_id}"),
            chat_id: "oc_sdk_loop".to_owned(),
            chat_type: RemoteFeishuChatType::P2p,
            sender_open_id: "ou_sender".to_owned(),
            mentions: vec!["ou_bot".to_owned()],
            text: "SDK loop 入站消息".to_owned(),
            media_refs: Vec::new(),
            message_kind: RemoteFeishuIncomingMessageKind::Text,
            reply_context: None,
            session_busy: false,
        }
    }

    fn card_action() -> RemoteFeishuCardActionEvent {
        RemoteFeishuCardActionEvent {
            session_id: SessionId(31),
            client_id: "feishu-sdk-loop".to_owned(),
            request_id: "sdk-loop-card-req".to_owned(),
            message_id: Some("om_sdk_loop_card".to_owned()),
            chat_id: "oc_sdk_loop".to_owned(),
            operator_open_id: "ou_operator".to_owned(),
            action_tag: "button".to_owned(),
            action_value_json: r#"{"axon_action":"clarify"}"#.to_owned(),
            reply_context: None,
            session_busy: true,
        }
    }

    fn message_callback(request_id: &str) -> RemoteFeishuGatewaySdkLoopMessageEvent {
        RemoteFeishuGatewaySdkLoopMessageEvent {
            request_id: request_id.to_owned(),
            message_id: format!("om_{request_id}"),
            source_created_at_ms: Some(100),
            chat_id: "oc_sdk_loop".to_owned(),
            chat_type: RemoteFeishuChatType::P2p,
            sender_open_id: "ou_sender".to_owned(),
            mentions: vec!["ou_bot".to_owned()],
            text: "SDK loop runner 入站消息".to_owned(),
            media_refs: Vec::new(),
            message_kind: RemoteFeishuIncomingMessageKind::Text,
            reply_context: None,
            session_busy: false,
        }
    }

    fn card_action_callback() -> RemoteFeishuGatewaySdkLoopCardActionEvent {
        RemoteFeishuGatewaySdkLoopCardActionEvent {
            request_id: "runner-card-req".to_owned(),
            message_id: Some("om_runner_card".to_owned()),
            chat_id: "oc_sdk_loop".to_owned(),
            operator_open_id: "ou_operator".to_owned(),
            action_tag: "button".to_owned(),
            action_value_json: r#"{"axon_action":"clarify"}"#.to_owned(),
            reply_context: None,
            session_busy: true,
        }
    }

    fn event_path(kind: &str) -> PathBuf {
        axon_test_support::test_temp_root().join(format!(
            "axon-feishu-sdk-loop-{kind}-{}.jsonl",
            unique_suffix()
        ))
    }

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    }
}
