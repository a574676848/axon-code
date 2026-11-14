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
    /// 事件所属 Workspace；缺省时由服务端解析默认 Workspace。
    ///
    /// 与 HTTP 信封同语义：外部发送方不填，自带工作区上下文的客户端才填。
    #[serde(default)]
    workspace_id: Option<String>,
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
            source_channel: self.source_channel,
            event_id: self.event_id,
            workspace_id: self.workspace_id.unwrap_or_default().trim().to_owned(),
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
