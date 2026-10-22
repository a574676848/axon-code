//! Feishu 出站消息类型规划。

use axon_domain::{AxonError, AxonResult};
use serde_json::Value;

use crate::RemoteNotificationRenderArtifact;

/// Feishu interactive 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE: &str = "interactive";
/// Feishu text 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_TEXT: &str = "text";
/// Feishu post 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST: &str = "post";
/// Feishu image 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_IMAGE: &str = "image";
/// Feishu file 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_FILE: &str = "file";
/// Feishu audio 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_AUDIO: &str = "audio";
/// Feishu media 消息类型。
pub const REMOTE_FEISHU_OUTBOUND_MSG_TYPE_MEDIA: &str = "media";

const MSG_TYPE_KEY: &str = "msg_type";
const CONTENT_KEY: &str = "content";
const TEXT_KEY: &str = "text";
const CONTENT_TYPE_TEXT: &str = "text/plain";
pub(crate) const CONTENT_TYPE_MARKDOWN: &str = "text/markdown";
const CONTENT_TYPE_JSON: &str = "application/json";
const CONTENT_TYPE_FEISHU_CARD: &str = "application/vnd.feishu.card+json";

/// Feishu 出站消息计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuOutboundMessagePlan {
    /// Feishu msg_type。
    pub msg_type: String,
    /// Feishu content JSON 字符串。
    pub content_json: String,
}

/// Feishu 出站消息 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuOutboundMessageRuntime;

impl RemoteFeishuOutboundMessageRuntime {
    /// 从 notification artifact 规划 Feishu OpenAPI message body。
    pub fn plan_from_artifact(
        artifact: &RemoteNotificationRenderArtifact,
    ) -> AxonResult<RemoteFeishuOutboundMessagePlan> {
        if artifact.payload.trim().is_empty() {
            return Err(AxonError::runtime_failure("Feishu 出站消息 payload 为空"));
        }
        let content_type = artifact.content_type.trim();
        if content_type == CONTENT_TYPE_FEISHU_CARD {
            return Ok(message(
                REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
                artifact.payload.clone(),
            ));
        }
        if let Ok(value) = serde_json::from_str::<Value>(&artifact.payload)
            && let Some(plan) = explicit_message_plan(&value)?
        {
            return Ok(plan);
        }
        match content_type {
            CONTENT_TYPE_TEXT => text_message(&artifact.payload),
            CONTENT_TYPE_MARKDOWN => post_message(&artifact.payload),
            CONTENT_TYPE_JSON => Ok(message(
                REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
                artifact.payload.clone(),
            )),
            _ => Ok(message(
                REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE,
                artifact.payload.clone(),
            )),
        }
    }
}

fn explicit_message_plan(value: &Value) -> AxonResult<Option<RemoteFeishuOutboundMessagePlan>> {
    let Some(msg_type) = value.get(MSG_TYPE_KEY).and_then(Value::as_str) else {
        return Ok(None);
    };
    let msg_type = msg_type.trim();
    if !matches!(
        msg_type,
        REMOTE_FEISHU_OUTBOUND_MSG_TYPE_TEXT
            | REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST
            | REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE
            | REMOTE_FEISHU_OUTBOUND_MSG_TYPE_IMAGE
            | REMOTE_FEISHU_OUTBOUND_MSG_TYPE_FILE
            | REMOTE_FEISHU_OUTBOUND_MSG_TYPE_AUDIO
            | REMOTE_FEISHU_OUTBOUND_MSG_TYPE_MEDIA
    ) {
        return Err(AxonError::runtime_failure("Feishu 出站消息类型不支持"));
    }
    let content = value
        .get(CONTENT_KEY)
        .ok_or(AxonError::runtime_failure("Feishu 出站消息缺少 content"))?;
    let content_json = if let Some(text) = content.as_str() {
        text.to_owned()
    } else {
        serde_json::to_string(content)
            .map_err(|_| AxonError::runtime_failure("Feishu 出站 content 序列化失败"))?
    };
    Ok(Some(message(msg_type, content_json)))
}

fn text_message(text: &str) -> AxonResult<RemoteFeishuOutboundMessagePlan> {
    let content = serde_json::json!({ TEXT_KEY: text }).to_string();
    Ok(message(REMOTE_FEISHU_OUTBOUND_MSG_TYPE_TEXT, content))
}

fn post_message(markdown: &str) -> AxonResult<RemoteFeishuOutboundMessagePlan> {
    let content = serde_json::json!({
        "zh_cn": {
            "content": [[{"tag": "md", "text": markdown}]]
        }
    })
    .to_string();
    Ok(message(REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST, content))
}

fn message(msg_type: &str, content_json: String) -> RemoteFeishuOutboundMessagePlan {
    RemoteFeishuOutboundMessagePlan {
        msg_type: msg_type.to_owned(),
        content_json,
    }
}

#[cfg(test)]
mod tests {
    use crate::{RemoteNotificationAdapterKind, RemoteTransportKind};

    use super::*;

    fn artifact(content_type: &str, payload: &str) -> RemoteNotificationRenderArtifact {
        RemoteNotificationRenderArtifact {
            adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
            transport: RemoteTransportKind::Feishu,
            content_type: content_type.to_owned(),
            payload: payload.to_owned(),
            network_send_allowed: true,
        }
    }

    #[test]
    fn outbound_keeps_interactive_card_default() {
        let plan = RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&artifact(
            CONTENT_TYPE_JSON,
            r#"{"schema":"2.0"}"#,
        ))
        .expect("plan");

        assert_eq!(plan.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_INTERACTIVE);
    }

    #[test]
    fn outbound_accepts_explicit_text_and_image_messages() {
        let text = RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&artifact(
            CONTENT_TYPE_JSON,
            r#"{"msg_type":"text","content":{"text":"完成"}}"#,
        ))
        .expect("text");
        let image = RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&artifact(
            CONTENT_TYPE_JSON,
            r#"{"msg_type":"image","content":{"image_key":"img_x"}}"#,
        ))
        .expect("image");

        assert_eq!(text.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_TEXT);
        assert!(text.content_json.contains(TEXT_KEY));
        assert_eq!(image.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_IMAGE);
        assert!(image.content_json.contains("image_key"));
    }

    #[test]
    fn outbound_accepts_file_audio_and_media_messages() {
        for msg_type in [
            REMOTE_FEISHU_OUTBOUND_MSG_TYPE_FILE,
            REMOTE_FEISHU_OUTBOUND_MSG_TYPE_AUDIO,
            REMOTE_FEISHU_OUTBOUND_MSG_TYPE_MEDIA,
        ] {
            let payload =
                format!(r#"{{"msg_type":"{msg_type}","content":{{"file_key":"file_x"}}}}"#);
            let plan = RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&artifact(
                CONTENT_TYPE_JSON,
                &payload,
            ))
            .expect("plan");

            assert_eq!(plan.msg_type, msg_type);
            assert!(plan.content_json.contains("file_key"));
        }
    }

    #[test]
    fn outbound_wraps_markdown_as_post() {
        let plan = RemoteFeishuOutboundMessageRuntime::plan_from_artifact(&artifact(
            CONTENT_TYPE_MARKDOWN,
            "**完成**",
        ))
        .expect("markdown");

        assert_eq!(plan.msg_type, REMOTE_FEISHU_OUTBOUND_MSG_TYPE_POST);
        assert!(plan.content_json.contains("md"));
    }
}
