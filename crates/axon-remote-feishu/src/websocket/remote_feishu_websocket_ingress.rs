//! Feishu WebSocket 入站事件归一化。
//!
//! 将文本消息和卡片按钮归一化为 Axon 本地 `RemoteProtocolRequest`，不直接依赖真实 SDK。

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::{
    REMOTE_CONTROL_COMMAND_PREFIX, REMOTE_PROTOCOL_VERSION, RemoteControlMessageInput,
    RemoteFeishuMediaRef, RemoteProtocolPayload, RemoteProtocolRequest,
};

/// Feishu 卡片动作标签最大字符数。
pub const REMOTE_FEISHU_CARD_ACTION_TAG_MAX_CHARS: usize = 64;
/// Feishu 卡片动作 JSON 最大字符数。
pub const REMOTE_FEISHU_CARD_ACTION_VALUE_MAX_CHARS: usize = 512;
/// Feishu 入站消息已归一化原因。
pub const REMOTE_FEISHU_WEBSOCKET_INGRESS_REASON_MESSAGE_NORMALIZED: &str =
    "remote_feishu_websocket_ingress:message_normalized";
/// Feishu 卡片动作已归一化原因。
pub const REMOTE_FEISHU_WEBSOCKET_INGRESS_REASON_CARD_ACTION_NORMALIZED: &str =
    "remote_feishu_websocket_ingress:card_action_normalized";

/// Feishu 入站消息文本分类值：普通文本。
pub const REMOTE_FEISHU_INCOMING_MESSAGE_KIND_TEXT: &str = "text";
/// Feishu 入站消息文本分类值：远控命令。
pub const REMOTE_FEISHU_INCOMING_MESSAGE_KIND_COMMAND: &str = "command";
/// Feishu 单聊类型编码。
pub const REMOTE_FEISHU_CHAT_TYPE_P2P: &str = "p2p";
/// Feishu 群聊类型编码。
pub const REMOTE_FEISHU_CHAT_TYPE_GROUP: &str = "group";
/// Feishu 未知会话类型编码。
pub const REMOTE_FEISHU_CHAT_TYPE_UNKNOWN: &str = "unknown";

/// Feishu 入站消息文本分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFeishuIncomingMessageKind {
    /// 普通文本。
    Text,
    /// 远控命令。
    Command,
}

impl RemoteFeishuIncomingMessageKind {
    /// 从已归一化文本识别 Axon 入站分类。
    #[must_use]
    pub fn from_text(text: &str) -> Self {
        if text.trim_start().starts_with(REMOTE_CONTROL_COMMAND_PREFIX) {
            return Self::Command;
        }
        Self::Text
    }

    /// 返回稳定的事件源编码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => REMOTE_FEISHU_INCOMING_MESSAGE_KIND_TEXT,
            Self::Command => REMOTE_FEISHU_INCOMING_MESSAGE_KIND_COMMAND,
        }
    }
}

/// Feishu 入站消息所在的会话类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFeishuChatType {
    /// 单聊。
    P2p,
    /// 群聊。
    Group,
    /// SDK payload 未提供或无法识别。
    Unknown,
}

impl RemoteFeishuChatType {
    /// 从 Feishu message.chat_type 映射会话类型。
    #[must_use]
    pub fn from_feishu_value(value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            Some(REMOTE_FEISHU_CHAT_TYPE_P2P) => Self::P2p,
            Some(_) => Self::Group,
            None => Self::Unknown,
        }
    }

    /// 从事件源稳定编码恢复会话类型。
    #[must_use]
    pub fn from_stable_value(value: Option<&str>) -> Option<Self> {
        match value.map(str::trim).filter(|value| !value.is_empty()) {
            Some(REMOTE_FEISHU_CHAT_TYPE_P2P) => Some(Self::P2p),
            Some(REMOTE_FEISHU_CHAT_TYPE_GROUP) => Some(Self::Group),
            Some(REMOTE_FEISHU_CHAT_TYPE_UNKNOWN) | None => Some(Self::Unknown),
            Some(_) => None,
        }
    }

    /// 返回事件源使用的稳定编码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P2p => REMOTE_FEISHU_CHAT_TYPE_P2P,
            Self::Group => REMOTE_FEISHU_CHAT_TYPE_GROUP,
            Self::Unknown => REMOTE_FEISHU_CHAT_TYPE_UNKNOWN,
        }
    }

    /// 判断该类型是否需要执行群聊 mention 门禁。
    #[must_use]
    pub const fn requires_group_mention(self) -> bool {
        !matches!(self, Self::P2p)
    }
}

/// Feishu 文本消息事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuIncomingMessage {
    /// 目标会话。
    pub session_id: SessionId,
    /// 远控 client 标识。
    pub client_id: String,
    /// 请求 ID。
    pub request_id: String,
    /// Feishu message_id。
    pub message_id: String,
    /// Feishu chat 标识。
    pub chat_id: String,
    /// Feishu 会话类型。
    pub chat_type: RemoteFeishuChatType,
    /// 发送者 open_id。
    pub sender_open_id: String,
    /// 被 @ 的 open_id 列表。
    pub mentions: Vec<String>,
    /// 发送的文本。
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

/// Feishu reply/thread 上下文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuReplyContext {
    /// 被回复的消息 ID。
    pub reply_to_message_id: Option<String>,
    /// 被回复消息的有界文本。
    pub reply_to_text: Option<String>,
    /// 话题或 thread ID。
    pub thread_id: Option<String>,
}

/// Feishu 卡片按钮事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuCardActionEvent {
    /// 目标会话。
    pub session_id: SessionId,
    /// 远控 client 标识。
    pub client_id: String,
    /// 请求或 token ID。
    pub request_id: String,
    /// 被点击卡片自身的消息 ID；卡片动作回调携带，缺失时为空。
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

/// Feishu WebSocket 入站归一化 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuWebSocketIngressRuntime;

impl RemoteFeishuWebSocketIngressRuntime {
    /// 归一化文本消息事件。
    ///
    /// # Errors
    ///
    /// 当 session、client/request 或文本无效时返回错误。
    pub fn normalize_message(
        event: &RemoteFeishuIncomingMessage,
    ) -> AxonResult<RemoteProtocolRequest> {
        validate_message_event(event)?;
        Ok(RemoteProtocolRequest {
            protocol_version: REMOTE_PROTOCOL_VERSION.to_owned(),
            client_id: event.client_id.clone(),
            request_id: event.request_id.clone(),
            payload: RemoteProtocolPayload::DeliverMessage {
                message: RemoteControlMessageInput {
                    session_id: event.session_id,
                    delivery_id: event.chat_id.clone(),
                    raw_text: agent_visible_message_text(event),
                    session_busy: event.session_busy,
                    seen_delivery_ids: Vec::new(),
                },
            },
        })
    }

    /// 归一化卡片按钮事件。
    ///
    /// # Errors
    ///
    /// 当动作标签、动作值或协议字段无效时返回错误。
    pub fn normalize_card_action(
        event: &RemoteFeishuCardActionEvent,
    ) -> AxonResult<RemoteProtocolRequest> {
        validate_card_action_event(event)?;
        let raw_text = normalized_card_action_text(event);
        Ok(RemoteProtocolRequest {
            protocol_version: REMOTE_PROTOCOL_VERSION.to_owned(),
            client_id: event.client_id.clone(),
            request_id: event.request_id.clone(),
            payload: RemoteProtocolPayload::DeliverMessage {
                message: RemoteControlMessageInput {
                    session_id: event.session_id,
                    delivery_id: event.request_id.clone(),
                    raw_text,
                    session_busy: event.session_busy,
                    seen_delivery_ids: Vec::new(),
                },
            },
        })
    }

    /// 判断文本消息是否显式 @ 了机器人。
    #[must_use]
    pub fn message_mentions_bot(event: &RemoteFeishuIncomingMessage, bot_open_id: &str) -> bool {
        if bot_open_id.trim().is_empty() {
            return false;
        }
        event.mentions.iter().any(|mention| mention == bot_open_id)
    }
}

fn normalized_card_action_text(event: &RemoteFeishuCardActionEvent) -> String {
    let payload = event.action_value_json.trim();
    let mut raw_text = format!("/card {}", event.action_tag.trim());
    if !payload.is_empty() {
        raw_text.push(' ');
        raw_text.push_str(payload);
    }
    raw_text
}

fn agent_visible_message_text(event: &RemoteFeishuIncomingMessage) -> String {
    event.text.clone()
}

fn validate_message_event(event: &RemoteFeishuIncomingMessage) -> AxonResult<()> {
    if event.session_id.0 == 0
        || event.client_id.trim().is_empty()
        || event.request_id.trim().is_empty()
        || event.message_id.trim().is_empty()
        || event.chat_id.trim().is_empty()
        || event.sender_open_id.trim().is_empty()
        || event.text.trim().is_empty()
        || invalid_reply_context(event.reply_context.as_ref())
    {
        return Err(AxonError::runtime_failure(
            "Feishu WebSocket 入站消息事件无效",
        ));
    }
    Ok(())
}

fn validate_card_action_event(event: &RemoteFeishuCardActionEvent) -> AxonResult<()> {
    if event.session_id.0 == 0
        || event.client_id.trim().is_empty()
        || event.request_id.trim().is_empty()
        || event.chat_id.trim().is_empty()
        || event.operator_open_id.trim().is_empty()
        || event.action_tag.trim().is_empty()
        || event.action_tag.chars().count() > REMOTE_FEISHU_CARD_ACTION_TAG_MAX_CHARS
        || event.action_value_json.chars().count() > REMOTE_FEISHU_CARD_ACTION_VALUE_MAX_CHARS
        || invalid_reply_context(event.reply_context.as_ref())
    {
        return Err(AxonError::runtime_failure(
            "Feishu WebSocket 卡片动作事件无效",
        ));
    }
    Ok(())
}

fn invalid_reply_context(reply_context: Option<&RemoteFeishuReplyContext>) -> bool {
    let Some(reply_context) = reply_context else {
        return false;
    };
    reply_context
        .reply_to_message_id
        .as_deref()
        .is_some_and(str::is_empty)
        || reply_context
            .reply_to_text
            .as_deref()
            .is_some_and(str::is_empty)
        || reply_context
            .thread_id
            .as_deref()
            .is_some_and(str::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feishu_websocket_ingress_normalizes_text_message() {
        let request =
            RemoteFeishuWebSocketIngressRuntime::normalize_message(&RemoteFeishuIncomingMessage {
                session_id: SessionId(15),
                client_id: "feishu-client".to_owned(),
                request_id: "req-1".to_owned(),
                message_id: "om_msg_1".to_owned(),
                chat_id: "oc_xxx".to_owned(),
                chat_type: RemoteFeishuChatType::Group,
                sender_open_id: "ou_xxx".to_owned(),
                mentions: vec!["ou_bot".to_owned()],
                text: "帮我看一下当前状态".to_owned(),
                media_refs: Vec::new(),
                message_kind: RemoteFeishuIncomingMessageKind::Text,
                reply_context: None,
                session_busy: false,
            })
            .expect("normalize message");

        assert_eq!(request.protocol_version, REMOTE_PROTOCOL_VERSION);
        match request.payload {
            RemoteProtocolPayload::DeliverMessage { message } => {
                assert_eq!(message.raw_text, "帮我看一下当前状态");
                assert_eq!(message.delivery_id, "oc_xxx");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_ingress_keeps_command_reply_text_executable() {
        let request =
            RemoteFeishuWebSocketIngressRuntime::normalize_message(&RemoteFeishuIncomingMessage {
                session_id: SessionId(15),
                client_id: "feishu-client".to_owned(),
                request_id: "req-reply-command".to_owned(),
                message_id: "om_reply_command".to_owned(),
                chat_id: "oc_reply".to_owned(),
                chat_type: RemoteFeishuChatType::Group,
                sender_open_id: "ou_xxx".to_owned(),
                mentions: vec!["ou_bot".to_owned()],
                text: "/status".to_owned(),
                media_refs: Vec::new(),
                message_kind: RemoteFeishuIncomingMessageKind::Command,
                reply_context: Some(RemoteFeishuReplyContext {
                    reply_to_message_id: Some("om_parent_1".to_owned()),
                    reply_to_text: Some("上一条问题".to_owned()),
                    thread_id: Some("root_1".to_owned()),
                }),
                session_busy: false,
            })
            .expect("normalize command reply message");

        match request.payload {
            RemoteProtocolPayload::DeliverMessage { message } => {
                assert_eq!(message.raw_text, "/status");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_ingress_keeps_reply_body_as_current_message_only() {
        let request =
            RemoteFeishuWebSocketIngressRuntime::normalize_message(&RemoteFeishuIncomingMessage {
                session_id: SessionId(15),
                client_id: "feishu-client".to_owned(),
                request_id: "req-reply-1".to_owned(),
                message_id: "om_reply_1".to_owned(),
                chat_id: "oc_reply".to_owned(),
                chat_type: RemoteFeishuChatType::Group,
                sender_open_id: "ou_xxx".to_owned(),
                mentions: vec!["ou_bot".to_owned()],
                text: "这里继续处理".to_owned(),
                media_refs: Vec::new(),
                message_kind: RemoteFeishuIncomingMessageKind::Text,
                reply_context: Some(RemoteFeishuReplyContext {
                    reply_to_message_id: Some("om_parent_1".to_owned()),
                    reply_to_text: Some("上一条问题".to_owned()),
                    thread_id: Some("root_1".to_owned()),
                }),
                session_busy: false,
            })
            .expect("normalize reply message");

        match request.payload {
            RemoteProtocolPayload::DeliverMessage { message } => {
                assert_eq!(message.raw_text, "这里继续处理");
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_ingress_keeps_card_action_as_plain_input() {
        let request = RemoteFeishuWebSocketIngressRuntime::normalize_card_action(
            &RemoteFeishuCardActionEvent {
                session_id: SessionId(16),
                client_id: "feishu-client".to_owned(),
                request_id: "token-1".to_owned(),
                message_id: None,
                chat_id: "oc_card_1".to_owned(),
                operator_open_id: "ou_operator".to_owned(),
                action_tag: "button".to_owned(),
                action_value_json: r#"{"axon_action":"clarify"}"#.to_owned(),
                reply_context: None,
                session_busy: true,
            },
        )
        .expect("normalize card action");

        match request.payload {
            RemoteProtocolPayload::DeliverMessage { message } => {
                assert_eq!(
                    message.raw_text,
                    r#"/card button {"axon_action":"clarify"}"#
                );
                assert!(message.session_busy);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_ingress_keeps_empty_card_action_payload() {
        let request = RemoteFeishuWebSocketIngressRuntime::normalize_card_action(
            &RemoteFeishuCardActionEvent {
                session_id: SessionId(17),
                client_id: "feishu-client".to_owned(),
                request_id: "token-2".to_owned(),
                message_id: None,
                chat_id: "oc_card_2".to_owned(),
                operator_open_id: "ou_operator".to_owned(),
                action_tag: "button".to_owned(),
                action_value_json: String::new(),
                reply_context: None,
                session_busy: false,
            },
        )
        .expect("normalize card action");

        match request.payload {
            RemoteProtocolPayload::DeliverMessage { message } => {
                assert_eq!(message.raw_text, "/card button");
                assert!(!message.session_busy);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_ingress_keeps_clarify_card_action_as_plain_input() {
        let request = RemoteFeishuWebSocketIngressRuntime::normalize_card_action(
            &RemoteFeishuCardActionEvent {
                session_id: SessionId(20),
                client_id: "feishu-client".to_owned(),
                request_id: "token-clarify".to_owned(),
                message_id: None,
                chat_id: "oc_card_control".to_owned(),
                operator_open_id: "ou_operator".to_owned(),
                action_tag: "button".to_owned(),
                action_value_json: r#"{"action":"clarify"}"#.to_owned(),
                reply_context: None,
                session_busy: false,
            },
        )
        .expect("normalize card action");

        match request.payload {
            RemoteProtocolPayload::DeliverMessage { message } => {
                assert_eq!(message.raw_text, r#"/card button {"action":"clarify"}"#);
            }
            other => panic!("unexpected payload: {other:?}"),
        }
    }

    #[test]
    fn feishu_websocket_ingress_detects_bot_mention() {
        let mentioned = RemoteFeishuWebSocketIngressRuntime::message_mentions_bot(
            &RemoteFeishuIncomingMessage {
                session_id: SessionId(18),
                client_id: "feishu-client".to_owned(),
                request_id: "req-18".to_owned(),
                message_id: "om_msg_18".to_owned(),
                chat_id: "oc_xxx".to_owned(),
                chat_type: RemoteFeishuChatType::Group,
                sender_open_id: "ou_user".to_owned(),
                mentions: vec!["ou_bot".to_owned()],
                text: "请处理".to_owned(),
                media_refs: Vec::new(),
                message_kind: RemoteFeishuIncomingMessageKind::Text,
                reply_context: None,
                session_busy: false,
            },
            "ou_bot",
        );

        assert!(mentioned);
    }

    #[test]
    fn feishu_incoming_message_kind_classifies_command_after_normalization() {
        assert_eq!(
            RemoteFeishuIncomingMessageKind::from_text(" /status"),
            RemoteFeishuIncomingMessageKind::Command
        );
        assert_eq!(
            RemoteFeishuIncomingMessageKind::from_text("继续推进"),
            RemoteFeishuIncomingMessageKind::Text
        );
    }
}
