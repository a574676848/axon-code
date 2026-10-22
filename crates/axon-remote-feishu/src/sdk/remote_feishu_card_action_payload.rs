//! Feishu SDK 卡片动作 payload 到 gateway runner 事件的映射。
//!
//! 独立成模块的原因：按钮载荷（`action.value` / `action.form_value`）、被点击卡片自身的
//! `context.open_message_id`、操作人身份与幂等键派生属于同一职责；`remote_feishu_gateway_sdk_payload.rs`
//! 已接近单文件行数硬限，继续堆叠会破坏可读性。

use axon_crypto::Sha256Hex;
use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::{RemoteFeishuGatewaySdkLoopCardActionEvent, RemoteFeishuGatewaySdkLoopEvent};

use super::remote_feishu_gateway_sdk_payload::{
    ACTION_OBJECT_KEY, CHAT_ID_KEY, CONTEXT_OBJECT_KEY, EVENT_ID_KEY, EVENT_OBJECT_KEY,
    OPEN_CHAT_ID_KEY, OPEN_ID_KEY, OPERATOR_OBJECT_KEY, REQUEST_ID_KEY,
    RemoteFeishuGatewaySdkPayloadMapperRuntime, TAG_KEY, TOKEN_KEY, VALUE_KEY,
    first_optional_string, first_required_string, operator_user_id_object, parse_payload,
    reply_context,
};

/// 卡片动作默认标签：飞书按钮回调未声明 `tag` 时的等价语义。
const DEFAULT_CARD_ACTION_TAG: &str = "button";
/// 卡片动作幂等键前缀。
const CARD_ACTION_REQUEST_ID_PREFIX: &str = "card-action";
/// 表单类卡片动作的字段值对象。
const FORM_VALUE_KEY: &str = "form_value";
/// 被点击卡片自身的消息 ID。
const OPEN_MESSAGE_ID_KEY: &str = "open_message_id";

impl RemoteFeishuGatewaySdkPayloadMapperRuntime {
    /// 映射 Feishu SDK 卡片动作 payload。
    ///
    /// 幂等键由「基础 token + 操作人 + 动作标签 + 载荷」共同派生：同一张卡片上不同
    /// 操作人的点击是两条独立事实，去重键必须能区分它们。
    ///
    /// # Errors
    ///
    /// 当 payload JSON 无效或必要字段缺失时返回错误。
    pub fn map_card_action_payload(
        raw_payload: &str,
    ) -> AxonResult<RemoteFeishuGatewaySdkLoopEvent> {
        let root = parse_payload(raw_payload)?;
        let event = root.get(EVENT_OBJECT_KEY).unwrap_or(&root);
        let action = event.get(ACTION_OBJECT_KEY).unwrap_or(event);
        let context = event.get(CONTEXT_OBJECT_KEY).unwrap_or(event);
        let request_id = first_required_string(&[
            event.get(TOKEN_KEY),
            root.get(TOKEN_KEY),
            event.get(REQUEST_ID_KEY),
            root.get(REQUEST_ID_KEY),
            event.get(EVENT_ID_KEY),
        ])?;
        let chat_id = first_required_string(&[
            context.get(OPEN_CHAT_ID_KEY),
            event.get(CHAT_ID_KEY),
            root.get(CHAT_ID_KEY),
            action.get(CHAT_ID_KEY),
        ])?;
        let operator = event.get(OPERATOR_OBJECT_KEY).unwrap_or(event);
        let operator_user = operator_user_id_object(operator);
        let operator_open_id = first_required_string(&[
            operator_user.get(OPEN_ID_KEY),
            operator.get(OPEN_ID_KEY),
            event.get(OPEN_ID_KEY),
        ])?;
        let action_tag = first_optional_string(&[action.get(TAG_KEY), event.get(TAG_KEY)])
            .unwrap_or_else(|| DEFAULT_CARD_ACTION_TAG.to_owned());
        let action_value_json = action_value_json(action)?;
        let request_id = card_action_request_id(
            &request_id,
            &operator_open_id,
            &action_tag,
            &action_value_json,
        );
        Ok(RemoteFeishuGatewaySdkLoopEvent::CardAction(
            RemoteFeishuGatewaySdkLoopCardActionEvent {
                request_id,
                message_id: first_optional_string(&[
                    context.get(OPEN_MESSAGE_ID_KEY),
                    event.get(OPEN_MESSAGE_ID_KEY),
                    root.get(OPEN_MESSAGE_ID_KEY),
                ]),
                chat_id,
                operator_open_id,
                action_tag,
                action_value_json,
                reply_context: reply_context(event),
                session_busy: false,
            },
        ))
    }
}

/// 取按钮载荷：优先 `action.value`，其次表单提交的 `action.form_value`。
///
/// 表单卡片（含问答卡的分片形态）把字段值放在 `form_value` 里，只读 `value` 会得到
/// 空载荷，答案因此无法定位。
fn action_value_json(action: &Value) -> AxonResult<String> {
    let Some(value) = action.get(VALUE_KEY).or_else(|| action.get(FORM_VALUE_KEY)) else {
        return Ok(String::new());
    };
    if let Some(text) = value.as_str() {
        return Ok(text.to_owned());
    }
    serde_json::to_string(value)
        .map_err(|_| AxonError::runtime_failure("Feishu SDK cardAction value 序列化失败"))
}

fn card_action_request_id(
    base_id: &str,
    operator_open_id: &str,
    action_tag: &str,
    action_value_json: &str,
) -> String {
    let mut digest = Sha256Hex::new();
    digest.update(base_id.as_bytes());
    digest.update(b"\0");
    digest.update(operator_open_id.as_bytes());
    digest.update(b"\0");
    digest.update(action_tag.as_bytes());
    digest.update(b"\0");
    digest.update(action_value_json.as_bytes());
    format!(
        "{CARD_ACTION_REQUEST_ID_PREFIX}:{}:{}",
        base_id.trim(),
        digest.finalize_hex()
    )
}
