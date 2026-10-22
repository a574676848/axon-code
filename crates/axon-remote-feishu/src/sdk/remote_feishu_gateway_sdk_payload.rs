//! Feishu SDK 原始 payload 到 gateway SDK loop runner 事件的 mapper。
//!
//! 本模块只负责把真实 SDK 回调拿到的 JSON payload 映射为 runner shell 事件结构，
//! 不写事件源、不执行治理链，也不依赖真实 SDK crate。卡片动作的载荷解析见
//! `remote_feishu_card_action_payload.rs`。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::{
    RemoteFeishuChatType, RemoteFeishuGatewaySdkLoopEvent, RemoteFeishuGatewaySdkLoopMessageEvent,
    RemoteFeishuGatewaySdkPayloadTextContext, RemoteFeishuIncomingMessageKind,
    RemoteFeishuMediaKind, RemoteFeishuMediaRef, RemoteFeishuReplyContext,
    remote_feishu_gateway_sdk_marker_content::{
        parse_calendar_content, parse_folder_content, parse_hongbao_content,
        parse_location_content, parse_share_user_content, parse_system_content, parse_task_content,
        parse_video_chat_content, parse_vote_content,
    },
    remote_feishu_gateway_sdk_rich_content::{
        parse_interactive_content, parse_merge_forward_content, parse_share_chat_content,
    },
    remote_feishu_gateway_sdk_text::{mention_refs, normalize_message_text},
};

/// Feishu SDK message payload 已映射原因。
pub const REMOTE_FEISHU_GATEWAY_SDK_PAYLOAD_REASON_MESSAGE_MAPPED: &str =
    "remote_feishu_gateway_sdk_payload:message_mapped";
/// Feishu SDK cardAction payload 已映射原因。
pub const REMOTE_FEISHU_GATEWAY_SDK_PAYLOAD_REASON_CARD_ACTION_MAPPED: &str =
    "remote_feishu_gateway_sdk_payload:card_action_mapped";

pub(crate) const EVENT_OBJECT_KEY: &str = "event";
const EVENT_TYPE_KEY: &str = "event_type";
const HEADER_OBJECT_KEY: &str = "header";
const MESSAGE_OBJECT_KEY: &str = "message";
pub(crate) const ACTION_OBJECT_KEY: &str = "action";
pub(crate) const CONTEXT_OBJECT_KEY: &str = "context";
pub(crate) const OPERATOR_OBJECT_KEY: &str = "operator";
const REACTION_TYPE_OBJECT_KEY: &str = "reaction_type";
/// reaction 事件的用户动作时间（飞书事件标准字段，毫秒字符串）。
const ACTION_TIME_KEY: &str = "action_time";
const SENDER_OBJECT_KEY: &str = "sender";
const SENDER_ID_OBJECT_KEY: &str = "sender_id";
const OPERATOR_ID_OBJECT_KEY: &str = "operator_id";
pub(crate) const OPEN_ID_KEY: &str = "open_id";
pub(crate) const OPEN_CHAT_ID_KEY: &str = "open_chat_id";
const USER_ID_OBJECT_KEY: &str = "user_id";
const MENTIONS_KEY: &str = "mentions";
const TEXT_KEY: &str = "text";
const CONTENT_KEY: &str = "content";
pub(crate) const REQUEST_ID_KEY: &str = "request_id";
pub(crate) const EVENT_ID_KEY: &str = "event_id";
const MESSAGE_ID_KEY: &str = "message_id";
pub(crate) const CHAT_ID_KEY: &str = "chat_id";
const CHAT_TYPE_KEY: &str = "chat_type";
const EMOJI_TYPE_KEY: &str = "emoji_type";
pub(crate) const TOKEN_KEY: &str = "token";
pub(crate) const TAG_KEY: &str = "tag";
pub(crate) const VALUE_KEY: &str = "value";
const MESSAGE_TYPE_KEY: &str = "message_type";
const CREATE_TIME_KEY: &str = "create_time";
const PLAIN_TEXT_TYPE: &str = "text";
const POST_TYPE: &str = "post";
const IMAGE_TYPE: &str = "image";
const FILE_TYPE: &str = "file";
const AUDIO_TYPE: &str = "audio";
const MEDIA_TYPE: &str = "media";
const VIDEO_TYPE: &str = "video";
const STICKER_TYPE: &str = "sticker";
const FOLDER_TYPE: &str = "folder";
const HONGBAO_TYPE: &str = "hongbao";
const LOCATION_TYPE: &str = "location";
const MERGE_FORWARD_TYPE: &str = "merge_forward";
const SHARE_CHAT_TYPE: &str = "share_chat";
const SHARE_USER_TYPE: &str = "share_user";
const SYSTEM_TYPE: &str = "system";
const VOTE_TYPE: &str = "vote";
const VIDEO_CHAT_TYPE: &str = "video_chat";
const CALENDAR_TYPE: &str = "calendar";
const GENERAL_CALENDAR_TYPE: &str = "general_calendar";
const SHARE_CALENDAR_EVENT_TYPE: &str = "share_calendar_event";
const TASK_TYPE: &str = concat!("to", "do");
const INTERACTIVE_TYPE: &str = "interactive";
const CARD_TYPE: &str = "card";
const IMAGE_KEY: &str = "image_key";
const FILE_KEY: &str = "file_key";
const FILE_NAME_KEY: &str = "file_name";
const TITLE_KEY: &str = "title";
const TAG_KEY_POST: &str = "tag";
const REACTION_ACTION_ADDED: &str = "added";
const REACTION_ACTION_REMOVED: &str = "removed";
const REPLY_TO_MESSAGE_ID_KEY: &str = "reply_to_message_id";
const REPLY_TO_TEXT_KEY: &str = "reply_to_text";
const THREAD_ID_KEY: &str = "thread_id";
const ROOT_ID_KEY: &str = "root_id";
const PARENT_ID_KEY: &str = "parent_id";
const UPPER_MESSAGE_ID_KEY: &str = "upper_message_id";

/// Feishu SDK payload mapper runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuGatewaySdkPayloadMapperRuntime;

impl RemoteFeishuGatewaySdkPayloadMapperRuntime {
    /// 映射 Feishu SDK 文本消息 payload。
    ///
    /// # Errors
    ///
    /// 当 payload JSON 无效或必要字段缺失时返回错误。
    pub fn map_message_payload(raw_payload: &str) -> AxonResult<RemoteFeishuGatewaySdkLoopEvent> {
        Self::map_message_payload_with_text_context(raw_payload, None)
    }

    /// 映射 Feishu SDK 文本消息 payload，并按 bot 身份归一化文本。
    ///
    /// # Errors
    ///
    /// 当 payload JSON 无效或必要字段缺失时返回错误。
    pub fn map_message_payload_with_text_context(
        raw_payload: &str,
        text_context: Option<&RemoteFeishuGatewaySdkPayloadTextContext>,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopEvent> {
        let root = parse_payload(raw_payload)?;
        let event = root.get(EVENT_OBJECT_KEY).unwrap_or(&root);
        let message = event.get(MESSAGE_OBJECT_KEY).unwrap_or(event);
        let message_type = optional_string(message, MESSAGE_TYPE_KEY)
            .or_else(|| optional_string(event, MESSAGE_TYPE_KEY));
        let message_type = message_type.unwrap_or_else(|| PLAIN_TEXT_TYPE.to_owned());
        let request_id = first_required_string(&[
            message.get(MESSAGE_ID_KEY),
            event.get(MESSAGE_ID_KEY),
            root.get(MESSAGE_ID_KEY),
            event.get(REQUEST_ID_KEY),
            root.get(REQUEST_ID_KEY),
            event.get(EVENT_ID_KEY),
            root.get(EVENT_ID_KEY),
        ])?;
        let message_id = first_required_string(&[
            message.get(MESSAGE_ID_KEY),
            event.get(MESSAGE_ID_KEY),
            root.get(MESSAGE_ID_KEY),
        ])?;
        let source_created_at_ms = first_optional_u64(&[
            message.get(CREATE_TIME_KEY),
            event.get(CREATE_TIME_KEY),
            root.get(CREATE_TIME_KEY),
        ]);
        let chat_id = first_required_string(&[
            message.get(CHAT_ID_KEY),
            event.get(CHAT_ID_KEY),
            root.get(CHAT_ID_KEY),
        ])?;
        let chat_type = RemoteFeishuChatType::from_feishu_value(
            optional_string(message, CHAT_TYPE_KEY)
                .or_else(|| optional_string(event, CHAT_TYPE_KEY))
                .or_else(|| optional_string(&root, CHAT_TYPE_KEY))
                .as_deref(),
        );
        let sender = event.get(SENDER_OBJECT_KEY).unwrap_or(event);
        let sender_user = sender_user_id_object(sender);
        let sender_open_id = first_required_string(&[
            sender_user.get(OPEN_ID_KEY),
            sender.get(OPEN_ID_KEY),
            event.get(OPEN_ID_KEY),
        ])?;
        let parsed = parse_message_content(message, &message_type)?;
        let mention_refs = mention_refs(message.get(MENTIONS_KEY), text_context);
        let text = normalize_message_text(
            &parsed.text,
            &mention_refs,
            text_context.is_some() && message_type == PLAIN_TEXT_TYPE,
        );
        if text.trim().is_empty() {
            return Err(AxonError::runtime_failure(
                "Feishu SDK message payload 文本归一化后为空",
            ));
        }
        Ok(RemoteFeishuGatewaySdkLoopEvent::Message(
            RemoteFeishuGatewaySdkLoopMessageEvent {
                request_id,
                message_id,
                source_created_at_ms,
                chat_id,
                chat_type,
                sender_open_id,
                mentions: mention_refs
                    .iter()
                    .filter(|mention| !mention.open_id.is_empty())
                    .map(|mention| mention.open_id.clone())
                    .collect(),
                media_refs: parsed.media_refs,
                message_kind: RemoteFeishuIncomingMessageKind::from_text(&text),
                text,
                reply_context: reply_context(message).or_else(|| reply_context(event)),
                session_busy: false,
            },
        ))
    }

    /// 映射 Feishu SDK reaction payload。
    ///
    /// # Errors
    ///
    /// 当 payload JSON 无效或必要字段缺失时返回错误。
    pub fn map_reaction_payload(raw_payload: &str) -> AxonResult<RemoteFeishuGatewaySdkLoopEvent> {
        let root = parse_payload(raw_payload)?;
        let event = root.get(EVENT_OBJECT_KEY).unwrap_or(&root);
        let reaction = event.get(REACTION_TYPE_OBJECT_KEY).unwrap_or(event);
        let operator = event.get(OPERATOR_OBJECT_KEY).unwrap_or(event);
        let operator_user = operator_user_id_object(operator);
        let message_id = first_required_string(&[
            event.get(MESSAGE_ID_KEY),
            root.get(MESSAGE_ID_KEY),
            event.get(TOKEN_KEY),
        ])?;
        let chat_id = first_required_string(&[
            event.get(CHAT_ID_KEY),
            root.get(CHAT_ID_KEY),
            event
                .get(CONTEXT_OBJECT_KEY)
                .and_then(|ctx| ctx.get(OPEN_CHAT_ID_KEY)),
        ])?;
        let sender_open_id = first_required_string(&[
            operator_user.get(OPEN_ID_KEY),
            operator.get(OPEN_ID_KEY),
            event
                .get(USER_ID_OBJECT_KEY)
                .and_then(|user_id| user_id.get(OPEN_ID_KEY)),
            event
                .get(USER_ID_OBJECT_KEY)
                .and_then(|user_id| user_id.get(USER_ID_OBJECT_KEY)),
            event.get(OPEN_ID_KEY),
        ])?;
        let emoji_type = first_required_string(&[
            reaction.get(EMOJI_TYPE_KEY),
            event.get(EMOJI_TYPE_KEY),
            root.get(EMOJI_TYPE_KEY),
        ])?;
        let action = reaction_action(&root, event);
        let text = format!("reaction:{action}:{emoji_type}");
        // reaction 是「此刻发生的用户动作」，必须携带自有事件时间：缺失时会被 ingress
        // 水位按「缺少有效 sourceCreatedAtMs」直接确认并丢弃，reaction 路径曾因此完全不可用。
        let source_created_at_ms =
            first_optional_u64(&[event.get(ACTION_TIME_KEY), root.get(ACTION_TIME_KEY)]);
        // reaction payload 本身不含 chat_type，由补齐阶段注入；缺失时保持 Unknown，
        // 由上层拒绝处理，而不是把群聊默认记成私聊。
        let chat_type = RemoteFeishuChatType::from_feishu_value(
            optional_string(event, CHAT_TYPE_KEY)
                .or_else(|| optional_string(&root, CHAT_TYPE_KEY))
                .as_deref(),
        );
        Ok(RemoteFeishuGatewaySdkLoopEvent::Message(
            RemoteFeishuGatewaySdkLoopMessageEvent {
                request_id: reaction_request_id(event, &message_id, &action, &emoji_type),
                message_id,
                source_created_at_ms,
                chat_id,
                chat_type,
                sender_open_id,
                mentions: Vec::new(),
                media_refs: Vec::new(),
                message_kind: RemoteFeishuIncomingMessageKind::from_text(&text),
                text,
                reply_context: reply_context(event),
                session_busy: false,
            },
        ))
    }
}

pub(crate) fn parse_payload(raw_payload: &str) -> AxonResult<Value> {
    serde_json::from_str(raw_payload)
        .map_err(|_| AxonError::runtime_failure("Feishu SDK payload JSON 无效"))
}

pub(crate) fn first_required_string(candidates: &[Option<&Value>]) -> AxonResult<String> {
    first_optional_string(candidates).ok_or(AxonError::runtime_failure(
        "Feishu SDK payload 必要字段缺失",
    ))
}

pub(crate) fn first_optional_string(candidates: &[Option<&Value>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .filter_map(|value| value.as_str())
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(str::to_owned)
}

fn first_optional_u64(candidates: &[Option<&Value>]) -> Option<u64> {
    candidates.iter().flatten().find_map(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str()?.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
    })
}

pub(crate) fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn sender_user_id_object(sender: &Value) -> &Value {
    sender
        .get(SENDER_ID_OBJECT_KEY)
        .or_else(|| sender.get(USER_ID_OBJECT_KEY))
        .unwrap_or(sender)
}

pub(crate) fn operator_user_id_object(operator: &Value) -> &Value {
    operator
        .get(OPERATOR_ID_OBJECT_KEY)
        .or_else(|| operator.get(USER_ID_OBJECT_KEY))
        .unwrap_or(operator)
}

fn message_text(message: &Value) -> AxonResult<String> {
    if let Some(text) = optional_string(message, TEXT_KEY) {
        return Ok(text);
    }
    let content = optional_string(message, CONTENT_KEY).ok_or(AxonError::runtime_failure(
        "Feishu SDK message payload 文本缺失",
    ))?;
    let value = serde_json::from_str::<Value>(&content)
        .map_err(|_| AxonError::runtime_failure("Feishu SDK message content JSON 无效"))?;
    optional_string(&value, TEXT_KEY).ok_or(AxonError::runtime_failure(
        "Feishu SDK message content 文本缺失",
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedMessageContent {
    text: String,
    media_refs: Vec<RemoteFeishuMediaRef>,
}

pub(crate) fn visible_text_from_content(message_type: &str, content: &str) -> Option<String> {
    let parsed = match message_type {
        PLAIN_TEXT_TYPE => parse_text_content(content).ok(),
        POST_TYPE => parse_post_content(content).ok(),
        IMAGE_TYPE => parse_media_content(content, RemoteFeishuMediaKind::Image, IMAGE_KEY).ok(),
        FILE_TYPE => parse_media_content(content, RemoteFeishuMediaKind::File, FILE_KEY).ok(),
        AUDIO_TYPE => parse_media_content(content, RemoteFeishuMediaKind::Audio, FILE_KEY).ok(),
        MEDIA_TYPE | VIDEO_TYPE => {
            parse_media_content(content, RemoteFeishuMediaKind::Media, FILE_KEY).ok()
        }
        STICKER_TYPE => parse_media_content(content, RemoteFeishuMediaKind::Sticker, FILE_KEY).ok(),
        FOLDER_TYPE => parse_light_content(content, parse_folder_content).ok(),
        HONGBAO_TYPE => parse_light_content(content, parse_hongbao_content).ok(),
        LOCATION_TYPE => parse_light_content(content, parse_location_content).ok(),
        MERGE_FORWARD_TYPE => parse_rich_content(content, parse_merge_forward_content).ok(),
        SHARE_CHAT_TYPE => parse_rich_content(content, parse_share_chat_content).ok(),
        SHARE_USER_TYPE => parse_light_content(content, parse_share_user_content).ok(),
        SYSTEM_TYPE => parse_light_content(content, parse_system_content).ok(),
        VOTE_TYPE => parse_light_content(content, parse_vote_content).ok(),
        VIDEO_CHAT_TYPE => parse_light_content(content, parse_video_chat_content).ok(),
        CALENDAR_TYPE | GENERAL_CALENDAR_TYPE | SHARE_CALENDAR_EVENT_TYPE => {
            parse_light_content(content, parse_calendar_content).ok()
        }
        TASK_TYPE => parse_light_content(content, parse_task_content).ok(),
        INTERACTIVE_TYPE | CARD_TYPE => parse_rich_content(content, parse_interactive_content).ok(),
        _ => None,
    }?;
    let text = parsed.text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_owned())
}

fn parse_message_content(message: &Value, message_type: &str) -> AxonResult<ParsedMessageContent> {
    let content = optional_string(message, CONTENT_KEY).unwrap_or_default();
    match message_type {
        PLAIN_TEXT_TYPE => Ok(ParsedMessageContent {
            text: message_text(message)?,
            media_refs: Vec::new(),
        }),
        POST_TYPE => parse_post_content(&content),
        IMAGE_TYPE => parse_media_content(&content, RemoteFeishuMediaKind::Image, IMAGE_KEY),
        FILE_TYPE => parse_media_content(&content, RemoteFeishuMediaKind::File, FILE_KEY),
        AUDIO_TYPE => parse_media_content(&content, RemoteFeishuMediaKind::Audio, FILE_KEY),
        MEDIA_TYPE | VIDEO_TYPE => {
            parse_media_content(&content, RemoteFeishuMediaKind::Media, FILE_KEY)
        }
        STICKER_TYPE => parse_media_content(&content, RemoteFeishuMediaKind::Sticker, FILE_KEY),
        FOLDER_TYPE => parse_light_content(&content, parse_folder_content),
        HONGBAO_TYPE => parse_light_content(&content, parse_hongbao_content),
        LOCATION_TYPE => parse_light_content(&content, parse_location_content),
        MERGE_FORWARD_TYPE => parse_rich_content(&content, parse_merge_forward_content),
        SHARE_CHAT_TYPE => parse_rich_content(&content, parse_share_chat_content),
        SHARE_USER_TYPE => parse_light_content(&content, parse_share_user_content),
        SYSTEM_TYPE => parse_light_content(&content, parse_system_content),
        VOTE_TYPE => parse_light_content(&content, parse_vote_content),
        VIDEO_CHAT_TYPE => parse_light_content(&content, parse_video_chat_content),
        CALENDAR_TYPE | GENERAL_CALENDAR_TYPE | SHARE_CALENDAR_EVENT_TYPE => {
            parse_light_content(&content, parse_calendar_content)
        }
        TASK_TYPE => parse_light_content(&content, parse_task_content),
        INTERACTIVE_TYPE | CARD_TYPE => parse_rich_content(&content, parse_interactive_content),
        _ => Err(AxonError::runtime_failure(
            "Feishu SDK message payload 类型不支持",
        )),
    }
}

fn parse_text_content(content: &str) -> AxonResult<ParsedMessageContent> {
    let value = serde_json::from_str::<Value>(content)
        .map_err(|_| AxonError::runtime_failure("Feishu text content JSON 无效"))?;
    let text = optional_string(&value, TEXT_KEY)
        .ok_or(AxonError::runtime_failure("Feishu text content 文本缺失"))?;
    Ok(ParsedMessageContent {
        text,
        media_refs: Vec::new(),
    })
}

fn parse_post_content(content: &str) -> AxonResult<ParsedMessageContent> {
    let value = serde_json::from_str::<Value>(content)
        .map_err(|_| AxonError::runtime_failure("Feishu post content JSON 无效"))?;
    let mut parts = Vec::new();
    let mut media_refs = Vec::new();
    if let Some(title) = optional_string(&value, TITLE_KEY) {
        parts.push(title);
    }
    collect_post_elements(&value, &mut parts, &mut media_refs);
    Ok(ParsedMessageContent {
        text: parts.join("\n").trim().to_owned(),
        media_refs,
    })
}

fn collect_post_elements(
    value: &Value,
    parts: &mut Vec<String>,
    media_refs: &mut Vec<RemoteFeishuMediaRef>,
) {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_post_elements(item, parts, media_refs);
            }
        }
        Value::Object(map) => {
            let tag = map
                .get(TAG_KEY_POST)
                .and_then(Value::as_str)
                .unwrap_or_default();
            if tag == IMAGE_TYPE {
                push_media_ref(
                    map.get(IMAGE_KEY),
                    RemoteFeishuMediaKind::Image,
                    None,
                    media_refs,
                );
            } else if matches!(
                tag,
                FILE_TYPE | AUDIO_TYPE | MEDIA_TYPE | VIDEO_TYPE | STICKER_TYPE
            ) {
                let kind = match tag {
                    AUDIO_TYPE => RemoteFeishuMediaKind::Audio,
                    MEDIA_TYPE | VIDEO_TYPE => RemoteFeishuMediaKind::Media,
                    STICKER_TYPE => RemoteFeishuMediaKind::Sticker,
                    _ => RemoteFeishuMediaKind::File,
                };
                push_media_ref(
                    map.get(FILE_KEY),
                    kind,
                    map.get(FILE_NAME_KEY).and_then(Value::as_str),
                    media_refs,
                );
            }
            if let Some(text) = map.get(TEXT_KEY).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_owned());
                }
            }
            for child in map.values() {
                collect_post_elements(child, parts, media_refs);
            }
        }
        _ => {}
    }
}

fn parse_media_content(
    content: &str,
    kind: RemoteFeishuMediaKind,
    key_name: &str,
) -> AxonResult<ParsedMessageContent> {
    let value = serde_json::from_str::<Value>(content)
        .map_err(|_| AxonError::runtime_failure("Feishu media content JSON 无效"))?;
    let key = optional_string(&value, key_name).ok_or(AxonError::runtime_failure(
        "Feishu media content 缺少媒体 key",
    ))?;
    let file_name = optional_string(&value, FILE_NAME_KEY);
    let label = file_name.as_deref().unwrap_or(kind.as_str());
    Ok(ParsedMessageContent {
        text: format!("[Feishu {}: {label}]", kind.as_str()),
        media_refs: vec![RemoteFeishuMediaRef {
            kind,
            key,
            file_name,
        }],
    })
}

fn parse_rich_content(
    content: &str,
    parser: fn(&str) -> AxonResult<String>,
) -> AxonResult<ParsedMessageContent> {
    Ok(ParsedMessageContent {
        text: parser(content)?,
        media_refs: Vec::new(),
    })
}

fn parse_light_content(
    content: &str,
    parser: fn(&str) -> AxonResult<String>,
) -> AxonResult<ParsedMessageContent> {
    Ok(ParsedMessageContent {
        text: parser(content)?,
        media_refs: Vec::new(),
    })
}

fn push_media_ref(
    key: Option<&Value>,
    kind: RemoteFeishuMediaKind,
    file_name: Option<&str>,
    media_refs: &mut Vec<RemoteFeishuMediaRef>,
) {
    let Some(key) = key.and_then(Value::as_str).map(str::trim) else {
        return;
    };
    if key.is_empty() {
        return;
    }
    media_refs.push(RemoteFeishuMediaRef {
        kind,
        key: key.to_owned(),
        file_name: file_name
            .map(str::to_owned)
            .filter(|value| !value.is_empty()),
    });
}

fn reaction_action(root: &Value, event: &Value) -> String {
    let event_type = optional_string(root, EVENT_TYPE_KEY)
        .or_else(|| {
            root.get(HEADER_OBJECT_KEY)
                .and_then(|header| optional_string(header, EVENT_TYPE_KEY))
        })
        .unwrap_or_default();
    if event_type.contains("deleted")
        || optional_string(event, ACTION_OBJECT_KEY).as_deref() == Some(REACTION_ACTION_REMOVED)
    {
        return REACTION_ACTION_REMOVED.to_owned();
    }
    REACTION_ACTION_ADDED.to_owned()
}

fn reaction_request_id(event: &Value, message_id: &str, action: &str, emoji_type: &str) -> String {
    first_optional_string(&[
        event.get(REQUEST_ID_KEY),
        event.get(EVENT_ID_KEY),
        event.get(TOKEN_KEY),
    ])
    .unwrap_or_else(|| format!("{message_id}:{action}:{emoji_type}"))
}

pub(crate) fn reply_context(value: &Value) -> Option<RemoteFeishuReplyContext> {
    let context = RemoteFeishuReplyContext {
        reply_to_message_id: first_optional_string(&[
            value.get(REPLY_TO_MESSAGE_ID_KEY),
            value.get(PARENT_ID_KEY),
            value.get(UPPER_MESSAGE_ID_KEY),
            value.get(ROOT_ID_KEY),
        ]),
        reply_to_text: optional_string(value, REPLY_TO_TEXT_KEY),
        thread_id: first_optional_string(&[value.get(THREAD_ID_KEY), value.get(ROOT_ID_KEY)]),
    };
    if context.reply_to_message_id.is_none()
        && context.reply_to_text.is_none()
        && context.thread_id.is_none()
    {
        return None;
    }
    Some(context)
}
