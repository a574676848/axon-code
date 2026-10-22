//! Thread 历史中的结构化模型事实。

use axon_domain::{OutputRef, ToolCallInput};
use serde::{Deserialize, Serialize};

/// 工具调用或结果的持久化状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThreadHistoryStatus {
    /// 已创建但尚未完成执行。
    InProgress,
    /// 已成功完成。
    Completed,
    /// 执行失败。
    Failed,
    /// 被中断或取消。
    Interrupted,
    /// 被策略或审批阻断。
    Rejected,
}

/// 已持久化的工具执行回执。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHistoryReceipt {
    /// 回执状态。
    pub status: ThreadHistoryStatus,
    /// 面向模型的有界摘要。
    pub summary: String,
    /// 工具结构化元数据。
    pub metadata: Option<serde_json::Value>,
    /// 工具完整输出引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_ref: Option<OutputRef>,
    /// 失败或阻断类型。
    pub error_type: Option<String>,
    /// 是否允许远程触发。
    pub remote_trigger_allowed: bool,
    /// 是否发生文件系统写入。
    pub filesystem_write_performed: bool,
    /// 执行耗时。
    pub duration_ms: Option<u64>,
}

/// Thread 历史中的图片引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHistoryImageRef {
    /// 本地图片路径；archive-first 引用为空。
    pub path: String,
    /// MIME 类型。
    pub mime_type: String,
    /// 原始内容字节数。
    pub byte_len: Option<u64>,
    /// 原始内容 SHA-256。
    pub sha256: Option<String>,
    /// 可恢复的归档引用。
    pub archive_ref: Option<String>,
    /// 来源追踪引用。
    pub source_ref: Option<String>,
}

/// Thread 历史中的文件引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadHistoryFileRef {
    /// 文件显示名。
    pub name: String,
    /// 磁盘上的绝对路径；由本地文件登记时给出，恢复会话后模型仍能定位原文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// MIME 类型。
    pub mime_type: String,
    /// 有界安全预览。
    pub preview: String,
    /// 原始内容字节数。
    pub byte_len: Option<u64>,
    /// 原始内容 SHA-256。
    pub sha256: Option<String>,
    /// 可恢复的归档引用。
    pub archive_ref: Option<String>,
    /// 来源追踪引用。
    pub source_ref: Option<String>,
}

/// 一次 Turn 中可恢复的结构化历史 item。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ThreadHistoryItem {
    /// 用户输入。
    UserMessage {
        /// item 顺序。
        sequence: u64,
        /// transcript part 身份。
        part_id: String,
        /// 用户内容。
        content: String,
        /// 本轮图片引用。
        local_images: Vec<ThreadHistoryImageRef>,
        /// 本轮文件引用。
        local_files: Vec<ThreadHistoryFileRef>,
        /// Provider metadata。
        provider_metadata: Option<serde_json::Value>,
    },
    /// 助手文本响应。
    AssistantMessage {
        /// item 顺序。
        sequence: u64,
        /// transcript part 身份。
        part_id: String,
        /// 助手内容。
        content: String,
        /// Provider metadata。
        provider_metadata: Option<serde_json::Value>,
    },
    /// Provider 原生 reasoning item。
    Reasoning {
        /// item 顺序。
        sequence: u64,
        /// 产生该 reasoning 的 provider turn；用于恢复原始消息分组。
        provider_turn_id: u64,
        /// transcript part 身份。
        part_id: String,
        /// Provider reasoning ID。
        reasoning_id: String,
        /// reasoning 内容。
        content: String,
        /// Provider metadata。
        provider_metadata: Option<serde_json::Value>,
    },
    /// Provider 原生 tool-call item。
    ToolCall {
        /// item 顺序。
        sequence: u64,
        /// 产生该 tool-call 的 provider turn；同一值表示同一 assistant tool-call batch。
        provider_turn_id: u64,
        /// transcript part 身份。
        part_id: String,
        /// Axon runtime tool-call ID。
        tool_call_id: String,
        /// 执行账本 operation ID。
        operation_id: String,
        /// Provider tool-call ID。
        provider_call_id: String,
        /// Provider 工具名。
        tool_name: String,
        /// 协议无关的完整工具输入。
        input: ToolCallInput,
        /// 当前执行状态。
        status: ThreadHistoryStatus,
        /// Provider metadata。
        provider_metadata: Option<serde_json::Value>,
    },
    /// Provider 可见的非会话注入上下文（Workspace、Skills、环境、上下文快照等）。
    ///
    /// 逐字节保存当轮真正发给 Provider 的消息，供后续轮次按原位置回放：Provider 前缀
    /// 缓存只在会话内消息列表「只追加」时才可能跨轮命中，重新推导或改写这段内容都会
    /// 让整段会话历史失去缓存。
    InjectedContext {
        /// item 顺序。
        sequence: u64,
        /// 注入来源身份；仅用于运行时对账，不进入模型请求。
        part_id: String,
        /// 模型可见消息 JSON（`AgentModelMessage` 线格式）。
        message_json: String,
    },
    /// 工具执行结果 item。
    ToolResult {
        /// item 顺序。
        sequence: u64,
        /// 对应 tool-call 的 provider turn；用于恢复原始消息分组。
        provider_turn_id: u64,
        /// transcript part 身份。
        part_id: String,
        /// Axon runtime tool-call ID。
        tool_call_id: String,
        /// 执行账本 operation ID。
        operation_id: String,
        /// Provider tool-call ID。
        provider_call_id: String,
        /// 实际执行的工具名。
        tool_name: String,
        /// 工具结果内容。
        content: String,
        /// 当前执行状态。
        status: ThreadHistoryStatus,
        /// 已完成回执；InProgress 结果可以为空。
        receipt: Option<Box<ThreadHistoryReceipt>>,
        /// Provider metadata。
        provider_metadata: Option<serde_json::Value>,
    },
}

impl ThreadHistoryItem {
    /// 返回 item 顺序。
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        match self {
            Self::UserMessage { sequence, .. }
            | Self::AssistantMessage { sequence, .. }
            | Self::Reasoning { sequence, .. }
            | Self::ToolCall { sequence, .. }
            | Self::ToolResult { sequence, .. }
            | Self::InjectedContext { sequence, .. } => *sequence,
        }
    }

    /// 返回 transcript part 身份。
    #[must_use]
    pub fn part_id(&self) -> &str {
        match self {
            Self::UserMessage { part_id, .. }
            | Self::AssistantMessage { part_id, .. }
            | Self::Reasoning { part_id, .. }
            | Self::ToolCall { part_id, .. }
            | Self::ToolResult { part_id, .. }
            | Self::InjectedContext { part_id, .. } => part_id,
        }
    }

    /// 返回 provider turn 批次身份；文本 item 没有工具批次。
    #[must_use]
    pub const fn provider_turn_id(&self) -> Option<u64> {
        match self {
            Self::Reasoning {
                provider_turn_id, ..
            }
            | Self::ToolCall {
                provider_turn_id, ..
            }
            | Self::ToolResult {
                provider_turn_id, ..
            } => Some(*provider_turn_id),
            Self::UserMessage { .. }
            | Self::AssistantMessage { .. }
            | Self::InjectedContext { .. } => None,
        }
    }
}
