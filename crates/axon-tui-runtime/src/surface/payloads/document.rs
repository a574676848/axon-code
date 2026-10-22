//! 文档、消息、计划、审阅和错误 cell payload。

use std::{collections::BTreeMap, path::PathBuf};

use axon_protocol::diff::FileChange;
use serde::{Deserialize, Serialize};

use crate::surface::cell_payload::{CellPayload, CellPayloadKind};

/// Assistant/User message cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageCellPayload {
    /// 稳定 message part id。
    pub id: String,
    /// Markdown 或 streaming source。
    pub source: String,
    /// source 类型。
    pub source_kind: MessageSourceKindPayload,
}

/// Message source 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageSourceKindPayload {
    /// 稳定 Markdown。
    Markdown,
    /// 流式 source。
    Stream,
}

/// Diff cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffCellPayload {
    /// 稳定 diff part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 文件变更。
    pub changes: BTreeMap<PathBuf, FileChange>,
    /// 诊断信息。
    pub diagnostics: BTreeMap<PathBuf, Vec<String>>,
    /// 已应用文本写入数。
    pub text_writes_applied: usize,
    /// 已应用重命名数。
    pub renames_applied: usize,
    /// 是否截断。
    pub truncated: bool,
    /// 截断边界。
    pub truncation_boundary: Option<String>,
}

/// Plan cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanCellPayload {
    /// 稳定 plan part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 说明。
    pub explanation: Option<String>,
    /// 步骤。
    pub steps: Vec<PlanStepPayload>,
    /// source 文本。
    pub source: String,
    /// source 类型。
    pub source_kind: PlanSourceKindPayload,
}

/// Plan step payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanStepPayload {
    /// 状态。
    pub status: PlanStatusPayload,
    /// 摘要。
    pub summary: String,
}

/// Plan source 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanSourceKindPayload {
    /// 稳定 Markdown。
    Markdown,
    /// 流式 source。
    Stream,
}

/// Plan 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatusPayload {
    /// 待执行。
    Pending,
    /// 执行中。
    Running,
    /// 已完成。
    Completed,
    /// 失败。
    Failed,
    /// 跳过。
    Skipped,
}

/// Review cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewCellPayload {
    /// 稳定 review part id。
    pub id: String,
    /// 主题。
    pub subject: String,
    /// 状态。
    pub status: Option<ReviewStatusPayload>,
    /// 明细。
    pub details: Vec<ReviewDetailPayload>,
}

/// Review detail payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewDetailPayload {
    /// 明细类型。
    pub kind: ReviewDetailKindPayload,
    /// 文本。
    pub text: String,
}

/// Review detail 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDetailKindPayload {
    /// 命令。
    Command,
    /// 诊断。
    Diagnostic,
    /// 变更。
    Delta,
    /// 普通消息。
    Message,
}

/// Review 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatusPayload {
    /// 已批准。
    Approved,
    /// 已拒绝。
    Denied,
    /// 待处理。
    Pending,
    /// 运行中。
    Running,
    /// 已完成。
    Completed,
    /// 失败。
    Failed,
    /// 阻断。
    Blocked,
    /// 停止。
    Stopped,
    /// 未知。
    Unknown,
}

/// Error cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorCellPayload {
    /// 稳定 error part id。
    pub id: String,
    /// 标题。
    pub title: String,
    /// 用户可读消息。
    pub message: String,
    /// 详情。
    pub details: Vec<String>,
    /// 下一步建议。
    pub next_step: Option<String>,
    /// 稳定 severity 编码。
    pub severity: String,
    /// 稳定 action 编码。
    pub action: String,
    /// 稳定 cell kind 编码。
    pub cell_kind: String,
}

impl MessageCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("message");
}

impl DiffCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("diff");
}

impl PlanCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("plan");
}

impl ReviewCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("review");
}

impl ErrorCellPayload {
    /// 稳定 kind。
    pub const KIND: CellPayloadKind = CellPayloadKind::new("error");
}

impl CellPayload for MessageCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for DiffCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for PlanCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for ReviewCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl CellPayload for ErrorCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

/// Question cell payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionCellPayload {
    pub id: String,
    pub call_id: String,
    pub status: String,
    pub questions: Vec<axon_domain::UserQuestionItem>,
    pub answers: Vec<axon_domain::UserAnswerItem>,
    pub handoff: Option<String>,
    pub deadline_at_ms: i64,
}

impl CellPayload for QuestionCellPayload {
    fn kind(&self) -> CellPayloadKind {
        Self::KIND
    }
}

impl QuestionCellPayload {
    pub const KIND: CellPayloadKind = CellPayloadKind::new("question");
}

impl From<&crate::surface::cell_question_data::AgentRenderQuestionCell> for QuestionCellPayload {
    fn from(cell: &crate::surface::cell_question_data::AgentRenderQuestionCell) -> Self {
        Self {
            id: cell.id.clone(),
            call_id: cell.call_id.clone(),
            status: cell.status.clone(),
            questions: cell.questions.clone(),
            answers: cell.answers.clone(),
            handoff: cell.handoff.clone(),
            deadline_at_ms: cell.deadline_at_ms,
        }
    }
}
