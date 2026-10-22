//! Feishu `transportContext` 的唯一解析契约。
//!
//! Daemon 主 turn 的 transport context 由 Daemon 侧统一构造，出站（egress）与实时
//! 进度投影（progress）都必须按同一份契约解析。历史上两处各自实现解析：契约从
//! `kind` / `chatId` 演进为 `channel` / `destination` 后，进度投影被静默判定为
//! 非 Feishu 渠道，整轮只剩 turn 结束后的最终回复。本模块是该契约的唯一事实来源。

use serde_json::Value;

/// Feishu 渠道标识；`kind` 与 `channel` 是同一契约的两种字段名。
const FEISHU_TRANSPORT_KIND: &str = "feishu";
const FIELD_KIND: &str = "kind";
const FIELD_CHANNEL: &str = "channel";
const FIELD_CHAT_ID: &str = "chatId";
const FIELD_DESTINATION: &str = "destination";
const FIELD_SOURCE_MESSAGE_ID: &str = "sourceMessageId";
const FIELD_REPLY_TO_MESSAGE_ID: &str = "replyToMessageId";
const FIELD_DELIVERY_NOTIFICATION_ID: &str = "deliveryNotificationId";

/// 解析后的 Feishu transport context。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FeishuTransportContext {
    /// 目标对话 ID。
    pub(crate) chat_id: String,
    /// 入站消息 ID（已剥离 `destination:` 前缀）；系统投递派生的回合为空。
    pub(crate) source_message_id: Option<String>,
    /// 出站回帖目标；与入站消息 ID 同源。
    pub(crate) reply_to_message_id: Option<String>,
    /// 由持久收件箱投递项派生的回合标记。
    pub(crate) delivery_notification_id: Option<String>,
}

impl FeishuTransportContext {
    /// 投递项派生的主 turn 正文由持久收件箱统一出站，投影端必须让位以避免重复投递。
    #[must_use]
    pub(crate) fn is_delivery_derived(&self) -> bool {
        self.delivery_notification_id.is_some()
    }
}

/// 判断 transport context 是否指向 Feishu 渠道。
#[must_use]
pub(crate) fn is_feishu(value: &Value) -> bool {
    string_field(value, FIELD_KIND) == Some(FEISHU_TRANSPORT_KIND)
        || string_field(value, FIELD_CHANNEL) == Some(FEISHU_TRANSPORT_KIND)
}

/// 解析 Feishu transport context；非 Feishu 渠道返回 `None`。
pub(crate) fn parse(value: &Value) -> Result<Option<FeishuTransportContext>, String> {
    if !is_feishu(value) {
        return Ok(None);
    }
    let chat_id = optional_string(value, FIELD_CHAT_ID)
        .or_else(|| optional_string(value, FIELD_DESTINATION))
        .ok_or_else(|| "Feishu transportContext 缺少 chatId 或 destination".to_owned())?;
    let source_message_id =
        optional_string(value, FIELD_SOURCE_MESSAGE_ID).map(|id| strip_destination_prefix(&id));
    let reply_to_message_id = optional_string(value, FIELD_REPLY_TO_MESSAGE_ID)
        .map(|id| strip_destination_prefix(&id))
        .or_else(|| source_message_id.clone());
    Ok(Some(FeishuTransportContext {
        chat_id,
        source_message_id,
        reply_to_message_id,
        delivery_notification_id: optional_string(value, FIELD_DELIVERY_NOTIFICATION_ID),
    }))
}

/// 入站消息 ID 形如 `destination:message`；回帖与表情清理只接受真实消息 ID。
fn strip_destination_prefix(message_id: &str) -> String {
    message_id
        .split_once(':')
        .map_or_else(|| message_id.to_owned(), |(_, actual)| actual.to_owned())
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).map(str::trim)
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    string_field(value, key)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::{is_feishu, parse};
    use serde_json::json;

    /// Daemon 目标态 interaction 主 turn 的真实 transport context。
    #[test]
    fn target_state_interaction_context_is_feishu() {
        let context = json!({
            "channel": "feishu",
            "destination": "oc_chat_target",
            "sourceMessageId": "oc_chat_target:om_msg_target"
        });

        assert!(is_feishu(&context));
        let parsed = parse(&context)
            .expect("解析目标态上下文")
            .expect("Feishu 上下文");
        assert_eq!(parsed.chat_id, "oc_chat_target");
        assert_eq!(parsed.source_message_id.as_deref(), Some("om_msg_target"));
        assert_eq!(parsed.reply_to_message_id.as_deref(), Some("om_msg_target"));
        assert!(!parsed.is_delivery_derived());
    }

    #[test]
    fn legacy_kind_chat_id_context_stays_supported() {
        let context = json!({
            "kind": "feishu",
            "chatId": "oc_chat",
            "sourceMessageId": "om_message"
        });

        let parsed = parse(&context)
            .expect("解析历史上下文")
            .expect("Feishu 上下文");
        assert_eq!(parsed.chat_id, "oc_chat");
        assert_eq!(parsed.source_message_id.as_deref(), Some("om_message"));
    }

    #[test]
    fn delivery_derived_context_is_marked_for_projection_handoff() {
        let context = json!({
            "channel": "feishu",
            "destination": "oc_chat_target",
            "deliveryNotificationId": "notification-1"
        });

        let parsed = parse(&context)
            .expect("解析投递派生上下文")
            .expect("Feishu 上下文");
        assert!(parsed.is_delivery_derived());
        assert!(parsed.reply_to_message_id.is_none());
    }

    #[test]
    fn other_channels_are_not_feishu() {
        let context = json!({"channel": "tui", "destination": "local"});

        assert!(!is_feishu(&context));
        assert_eq!(parse(&context).expect("解析非 Feishu 上下文"), None);
    }

    #[test]
    fn missing_conversation_identity_is_rejected() {
        let context = json!({"channel": "feishu", "sourceMessageId": "om_message"});

        let error = parse(&context).expect_err("缺少对话身份必须报错");
        assert!(error.contains("chatId"));
    }
}
