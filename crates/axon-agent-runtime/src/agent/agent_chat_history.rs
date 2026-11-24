//! Agent Pane 聊天历史投影。

use serde::{Deserialize, Serialize};

use crate::AgentResponseProjection;

/// Agent Pane 聊天历史 schema version。
pub const AGENT_PANE_CHAT_HISTORY_SCHEMA_VERSION: u32 = 1;

/// Agent Pane 聊天历史记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPaneChatHistoryRecord {
    /// schema version。
    pub schema_version: u32,
    /// pane ID。
    pub pane_id: u64,
    /// RuntimeCell ID。
    pub cell_id: u64,
    /// prompt 有界预览。
    pub prompt_preview: String,
    /// prompt 完整字符数。
    pub prompt_chars: usize,
    /// 回答有界预览。
    pub response_preview: String,
    /// 回答完整字符数。
    pub response_chars: usize,
    /// 回答预览是否截断。
    pub response_truncated: bool,
    /// provider delta 数量。
    pub delta_count: usize,
    /// 回答来源引用。
    pub source_ref: String,
    /// 记录时间。
    pub recorded_at_ms: u64,
    /// 是否允许远控查看完整回答。
    pub remote_full_response_allowed: bool,
    /// 是否持久化 provider 原始 payload。
    pub provider_raw_payload_persisted: bool,
    /// 是否持久化 request body。
    pub request_body_persisted: bool,
}

impl AgentPaneChatHistoryRecord {
    /// 从回答投影构造聊天历史记录。
    #[must_use]
    pub fn from_response(
        pane_id: u64,
        prompt: &str,
        response: &AgentResponseProjection,
        delta_count: usize,
        recorded_at_ms: u64,
    ) -> Self {
        Self {
            schema_version: AGENT_PANE_CHAT_HISTORY_SCHEMA_VERSION,
            pane_id,
            cell_id: response.cell_id,
            prompt_preview: prompt.to_owned(),
            prompt_chars: prompt.chars().count(),
            response_preview: response.preview.clone(),
            response_chars: response.full_content_chars,
            response_truncated: false,
            delta_count,
            source_ref: response.source_ref.clone(),
            recorded_at_ms,
            remote_full_response_allowed: false,
            provider_raw_payload_persisted: false,
            request_body_persisted: false,
        }
    }
}
