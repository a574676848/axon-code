//! 状态、命令、生成状态和 reasoning cell payload。

use serde::{Deserialize, Serialize};

use crate::surface::cell_payload::{CellPayload, CellPayloadKind};

/// Status cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusCellPayload {
    /// 稳定 status part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 状态条目。
    pub items: Vec<StatusItemPayload>,
}

/// Command cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCellPayload {
    /// 稳定 command part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 命令条目。
    pub items: Vec<StatusItemPayload>,
}

/// Generation status cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationStatusCellPayload {
    /// 稳定 generation status part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 生命周期条目。
    pub items: Vec<LifecycleItemPayload>,
}

/// Reasoning cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningCellPayload {
    /// reasoning id。
    pub id: String,
    /// reasoning 文本。
    pub text: String,
    /// 是否已结束。
    pub done: bool,
    /// reasoning 耗时毫秒。
    pub elapsed_ms: Option<u64>,
}

/// Turn summary cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSummaryCellPayload {
    /// 稳定 turn summary part id。
    pub id: String,
    /// turn 耗时毫秒。
    pub duration_ms: Option<u64>,
    /// turn token 总量。
    pub total_tokens: Option<u64>,
    /// turn 缓存 token 总量。
    pub cache_tokens: Option<u64>,
    /// turn 非缓存输入 token。
    #[serde(default)]
    pub input_tokens: Option<u64>,
    /// turn 缓存读取命中 token。
    #[serde(default)]
    pub cached_input_tokens: Option<u64>,
    /// turn 输出 token。
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// turn 推理输出 token。
    #[serde(default)]
    pub reasoning_output_tokens: Option<u64>,
    /// 缓存读取占完整输入的命中率，单位为 basis points。
    #[serde(default)]
    pub cache_hit_rate_basis_points: Option<u16>,
    /// 最近 generation 平均首字耗时毫秒。
    #[serde(default)]
    pub first_token_avg_ms: Option<u64>,
    /// 最近 generation 输出速率，单位为 0.1 tok/s。
    #[serde(default)]
    pub generation_tps_x10: Option<u64>,
}

/// 状态条目 payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusItemPayload {
    /// 稳定类型。
    pub kind: String,
    /// 摘要。
    pub summary: String,
    /// 生命周期阶段。
    pub phase: Option<LifecyclePhasePayload>,
    /// indicator。
    pub indicator: Option<String>,
    /// 工具名。
    pub tool_name: Option<String>,
    /// 耗时毫秒。
    pub duration_ms: Option<u64>,
    /// turn id。
    pub turn_id: Option<u64>,
    /// 尝试序号。
    pub attempt_index: Option<usize>,
}

/// 生命周期条目 payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleItemPayload {
    /// 稳定 lifecycle event id。
    pub id: String,
    /// 稳定类型。
    pub kind: String,
    /// 摘要。
    pub summary: String,
    /// 生命周期阶段。
    pub phase: LifecyclePhasePayload,
    /// indicator。
    pub indicator: Option<String>,
    /// 工具名。
    pub tool_name: Option<String>,
    /// 耗时毫秒。
    pub duration_ms: Option<u64>,
    /// 尝试序号。
    pub attempt_index: Option<usize>,
}

/// 生命周期阶段 payload。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecyclePhasePayload {
    /// provider streaming。
    Streaming,
    /// 模型生成。
    Generation,
    /// 普通提示。
    Notice,
    /// 命令反馈。
    Command,
    /// 未知。
    Unknown,
}

impl StatusCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("status");
}

impl CommandCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("command");
}

impl GenerationStatusCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("generation_status");
}

impl ReasoningCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("reasoning");
}

impl TurnSummaryCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("turn_summary");
}

impl CellPayload for StatusCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for CommandCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for GenerationStatusCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for ReasoningCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for TurnSummaryCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}
