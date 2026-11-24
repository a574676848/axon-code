use serde::{Deserialize, Serialize};

use super::event::{
    WireAssistantMessage, WireFileChangeEvent, WireGovernanceStatus, WireLifecycleStatus,
    WirePlanUpdate, WireReasoningMessage, WireReviewStatus, WireRuntimeError, WireSchemaVersion,
    WireSessionRef, WireToolCall, WireToolResult, WireTurnSummary, WireUsageUpdate,
    WireUserMessage, WireUserQuestion,
};

/// TUI 三区域 resume snapshot。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireResumeSnapshot {
    /// schema 版本。
    pub schema_version: WireSchemaVersion,
    /// 会话引用。
    pub session: WireSessionRef,
    /// 顶部状态栏。
    pub status_bar: WireStatusBarState,
    /// 中央 viewport 状态。
    pub viewport: WireViewportSnapshot,
    /// 底部输入区状态。
    pub input_panel: WireInputPanelState,
    /// 中央结构化渲染历史。
    pub timeline: Vec<WireRenderTimelineItem>,
}

/// 顶部状态栏状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireStatusBarState {
    /// 主标题。
    pub title: String,
    /// 副标题。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    /// 当前 lifecycle 状态。
    pub lifecycle_status: WireLifecycleStatus,
    /// 工作目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// provider 名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// model 名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 当前选择的 skill。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_skill_name: Option<String>,
    /// usage 概览。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<WireUsageUpdate>,
}

/// 中央 viewport snapshot。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WireViewportSnapshot {
    /// 滚动锚点。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scroll_anchor: Option<WireScrollAnchor>,
    /// 当前聚焦交互。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_interaction: Option<WireFocusedInteraction>,
}

/// 中央区滚动锚点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireScrollAnchor {
    /// timeline part ID。
    pub part_id: String,
    /// part 内偏移。
    pub offset: u16,
}

/// 当前聚焦交互。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireFocusedInteraction {
    /// 交互类别。
    pub kind: String,
    /// 目标 ID。
    pub target_id: String,
}

/// 底部输入区域状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireInputPanelState {
    /// 输入模式。
    pub mode: WireInputMode,
    /// 草稿文本。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<String>,
}

/// 输入模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WireInputMode {
    /// 普通输入。
    Prompt,
    /// 命令输入。
    Command,
    /// 只读回放。
    ReadOnlyReplay,
}

/// 中央结构化渲染项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireRenderTimelineItem {
    /// part ID。
    pub part_id: String,
    /// 会话内序号。
    pub sequence: u64,
    /// 记录时间，Unix 毫秒。
    pub recorded_at_ms: i64,
    /// item 类型。
    pub kind: WireRenderTimelineItemKind,
}

/// 中央结构化渲染项类型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "camelCase")]
pub enum WireRenderTimelineItemKind {
    /// Assistant 消息。
    AssistantMessage(WireAssistantMessage),
    /// User 消息。
    UserMessage(WireUserMessage),
    /// 模型 reasoning 消息。
    ReasoningMessage(WireReasoningMessage),
    /// 工具调用。
    ToolCall(WireToolCall),
    /// 工具结果。
    ToolResult(WireToolResult),
    /// 文件变更。
    FileChange(WireFileChangeEvent),
    /// 计划。
    PlanUpdate(WirePlanUpdate),
    /// Review。
    ReviewStatus(WireReviewStatus),
    /// 后台治理。
    GovernanceStatus(WireGovernanceStatus),
    /// Usage。
    UsageUpdate(WireUsageUpdate),
    /// Turn summary。
    TurnSummary(WireTurnSummary),
    /// 错误。
    Error(WireRuntimeError),
    /// 用户提问。
    UserQuestion(WireUserQuestion),
}
