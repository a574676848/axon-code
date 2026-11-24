//! WebSocket `submit` 帧的入站事件载荷。
//!
//! 与 HTTP 信封同字段同语义：`model` 与 `reasoningEffort` 都只跟随该轮，不写回配置。

use serde::Deserialize;

use crate::application::ExternalEventEnvelope;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WsEventRequest {
    source_channel: String,
    event_id: String,
    observed_at_ms: u64,
    expires_at_ms: u64,
    content: String,
    reply_channel: String,
    reply_destination: String,
    /// 客户端已落地的本地附件绝对路径。
    #[serde(default)]
    local_files: Vec<String>,
    /// 客户端显式指定的本轮模型；与 HTTP 信封同语义。
    #[serde(default)]
    model: Option<String>,
    /// 客户端显式指定的本轮推理强度；与 HTTP 信封同语义。
    #[serde(default)]
    reasoning_effort: Option<String>,
}

impl WsEventRequest {
    pub(super) fn into_event(self) -> ExternalEventEnvelope {
        ExternalEventEnvelope {
            client: None,
            source_channel: self.source_channel,
            event_id: self.event_id,
            // 外部事件不对外暴露会话标识：统一落到服务端内部的固定会话。
            session_id: None,
            // 外部事件不对外暴露 Workspace：固定落到 Axon 根目录这一条。
            workspace_id: String::new(),
            observed_at_ms: self.observed_at_ms,
            expires_at_ms: self.expires_at_ms,
            content: self.content,
            title: None,
            correlation_key: None,
            reply_channel: self.reply_channel,
            reply_destination: self.reply_destination,
            attachments: self.local_files,
            model: self.model,
            reasoning_effort: self.reasoning_effort,
        }
    }
}
