//! 工具、MCP、Hook、历史和 usage cell payload。

use serde::{Deserialize, Serialize};

use crate::surface::cell_payload::{CellPayload, CellPayloadKind};

/// Exec cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecCellPayload {
    /// 稳定 exec part id。
    pub id: String,
    /// 工具名。
    pub tool_name: String,
    /// 主展示标签。
    pub display_label: String,
    /// 副展示目标。
    pub display_target: Option<String>,
    /// 原始工具名。
    pub raw_tool_name: String,
    /// 详情展示文本。
    pub display_detail: Option<String>,
    /// 执行状态。
    pub status: ExecStatusPayload,
    /// provider call id。
    pub provider_call_id: Option<String>,
    /// 参数预览。
    pub arguments_preview: Option<String>,
    /// 输出预览。
    pub output_preview: Option<String>,
    /// 摘要。
    pub summary: Option<String>,
    /// 耗时毫秒。
    pub duration_ms: Option<u64>,
}

/// Exec 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecStatusPayload {
    /// 运行中。
    Running,
    /// 已完成。
    Completed,
    /// 失败。
    Failed,
    /// 输出中。
    Output,
}

/// Usage cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCellPayload {
    /// 稳定 usage part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// token 总数。
    pub total_tokens: u64,
    /// token 总数显示文本。
    pub total: String,
    /// 输入 token。
    pub input_tokens: u64,
    /// 输入显示文本。
    pub input: String,
    /// cache read token。
    pub cache_read_input_tokens: u64,
    /// cache read 显示文本。
    pub cache_read: String,
    /// cache write token。
    pub cache_write_input_tokens: u64,
    /// cache write 显示文本。
    pub cache_write: String,
    /// 输出 token。
    pub output_tokens: u64,
    /// 输出显示文本。
    pub output: String,
    /// reasoning token。
    pub reasoning_output_tokens: u64,
    /// reasoning 显示文本。
    pub reasoning: String,
}

/// Hook cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookCellPayload {
    /// 稳定 hook part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// hook 条目。
    pub items: Vec<HookItemPayload>,
}

/// Hook 条目 payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookItemPayload {
    /// run id。
    pub run_id: String,
    /// 名称。
    pub name: String,
    /// 生命周期状态。
    pub status: LifecycleStatusPayload,
    /// indicator。
    pub indicator: Option<String>,
    /// 状态消息。
    pub status_message: Option<String>,
    /// 输出条目。
    pub entries: Vec<HookOutputEntryPayload>,
}

/// Hook 输出条目 payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookOutputEntryPayload {
    /// 输出类型。
    pub kind: HookOutputKindPayload,
    /// 文本。
    pub text: String,
}

/// Hook 输出类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookOutputKindPayload {
    /// 反馈。
    Feedback,
    /// 警告。
    Warning,
    /// 停止。
    Stop,
    /// 上下文。
    Context,
    /// 错误。
    Error,
}

/// MCP cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCellPayload {
    /// 稳定 MCP part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 空状态文本。
    pub empty_text: String,
    /// 数据来源。
    pub source: McpCellSourcePayload,
    /// MCP 条目。
    pub items: Vec<McpItemPayload>,
}

/// MCP 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum McpCellSourcePayload {
    /// 工具调用。
    ToolCall,
    /// 目录。
    Catalog,
}

/// MCP 条目 payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpItemPayload {
    /// provider call id。
    pub provider_call_id: Option<String>,
    /// server 名称。
    pub server: String,
    /// tool 名称。
    pub tool: Option<String>,
    /// 主展示标签。
    pub display_label: String,
    /// 副展示目标。
    pub display_target: Option<String>,
    /// 原始工具名。
    pub raw_tool_name: String,
    /// 详情展示文本。
    pub display_detail: Option<String>,
    /// 生命周期状态。
    pub status: LifecycleStatusPayload,
    /// 参数预览。
    pub arguments_preview: Option<String>,
    /// 输出预览。
    pub output_preview: Option<String>,
    /// 摘要。
    pub summary: Option<String>,
}

/// History cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryCellPayload {
    /// 稳定 history part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 空状态文本。
    pub empty_text: String,
    /// 历史条目。
    pub items: Vec<HistoryItemPayload>,
}

/// History item payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryItemPayload {
    /// 名称。
    pub name: String,
    /// 类型标签。
    pub kind_label: Option<String>,
    /// 状态标签。
    pub status_label: Option<String>,
}

/// 通用生命周期状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleStatusPayload {
    /// 已完成。
    Completed,
    /// 失败。
    Failed,
    /// 运行中。
    Running,
    /// 等待中。
    Pending,
    /// 阻断。
    Blocked,
    /// 已停止。
    Stopped,
    /// 未知。
    Unknown,
}

impl ExecCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("exec");
}

impl UsageCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("usage");
}

impl HookCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("hook");
}

impl McpCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("mcp");
}

impl HistoryCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("history");
}

impl CellPayload for ExecCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for UsageCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for HookCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for McpCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for HistoryCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}
