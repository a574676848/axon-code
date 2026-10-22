//! Transcript cell 到 surface payload schema 的单向投影。

use crate::surface::{
    cell_payload::{CellPayloadEnvelope, CellPayloadSchemaError},
    payloads::{
        CommandCellPayload, DiffCellPayload, ErrorCellPayload, ExecCellPayload, ExecStatusPayload,
        GenerationStatusCellPayload, HistoryCellPayload, HistoryItemPayload, HookCellPayload,
        HookItemPayload, HookOutputEntryPayload, HookOutputKindPayload, LifecycleItemPayload,
        LifecyclePhasePayload, LifecycleStatusPayload, McpCellPayload, McpCellSourcePayload,
        McpItemPayload, MessageCellPayload, MessageSourceKindPayload, PlanCellPayload,
        PlanSourceKindPayload, PlanStatusPayload, PlanStepPayload, QuestionCellPayload,
        ReasoningCellPayload, ReviewCellPayload, ReviewDetailKindPayload, ReviewDetailPayload,
        ReviewStatusPayload, StatusCellPayload, StatusItemPayload, TurnSummaryCellPayload,
        UsageCellPayload,
    },
};

use crate::AgentTuiLifecyclePhase;

use super::{
    cell::{
        RenderCellData, RenderCommandCell, RenderGenerationStatusCell, RenderLifecycleItem,
        RenderMessageCell, RenderMessageSourceKind, RenderReasoningCell, RenderStatusCell,
        RenderStatusItem,
    },
    cell_event_data, cell_plan_data, cell_review_data,
};

pub(crate) fn cell_payload_envelope(
    data: &RenderCellData,
) -> Result<CellPayloadEnvelope, CellPayloadSchemaError> {
    match data {
        RenderCellData::Diff(cell) => {
            CellPayloadEnvelope::from_payload(&DiffCellPayload::from(cell))
        }
        RenderCellData::Command(cell) => {
            CellPayloadEnvelope::from_payload(&CommandCellPayload::from(cell))
        }
        RenderCellData::Exec(cell) => {
            CellPayloadEnvelope::from_payload(&ExecCellPayload::from(cell))
        }
        RenderCellData::History(cell) => {
            CellPayloadEnvelope::from_payload(&HistoryCellPayload::from(cell))
        }
        RenderCellData::Hook(cell) => {
            CellPayloadEnvelope::from_payload(&HookCellPayload::from(cell))
        }
        RenderCellData::Mcp(cell) => CellPayloadEnvelope::from_payload(&McpCellPayload::from(cell)),
        RenderCellData::Message(_) => Err(CellPayloadSchemaError::message_source_required()),
        RenderCellData::Plan(cell) => {
            CellPayloadEnvelope::from_payload(&PlanCellPayload::from(cell))
        }
        RenderCellData::Review(cell) => {
            CellPayloadEnvelope::from_payload(&ReviewCellPayload::from(cell))
        }
        RenderCellData::Error(cell) => {
            CellPayloadEnvelope::from_payload(&ErrorCellPayload::from(cell))
        }
        RenderCellData::Status(cell) => {
            CellPayloadEnvelope::from_payload(&StatusCellPayload::from(cell))
        }
        RenderCellData::GenerationStatus(cell) => {
            CellPayloadEnvelope::from_payload(&GenerationStatusCellPayload::from(cell))
        }
        RenderCellData::Reasoning(cell) => {
            CellPayloadEnvelope::from_payload(&ReasoningCellPayload::from(cell))
        }
        RenderCellData::TurnSummary(cell) => {
            CellPayloadEnvelope::from_payload(&TurnSummaryCellPayload::from(cell))
        }
        RenderCellData::Usage(cell) => {
            CellPayloadEnvelope::from_payload(&UsageCellPayload::from(cell))
        }
        RenderCellData::Question(cell) => {
            CellPayloadEnvelope::from_payload(&QuestionCellPayload::from(cell))
        }
    }
}

pub(crate) fn message_cell_payload_envelope(
    cell: &RenderMessageCell,
    source: &str,
) -> Result<CellPayloadEnvelope, CellPayloadSchemaError> {
    CellPayloadEnvelope::from_payload(&MessageCellPayload {
        id: cell.id.clone(),
        source: source.to_owned(),
        source_kind: MessageSourceKindPayload::from(cell.source_kind),
    })
}

impl From<RenderMessageSourceKind> for MessageSourceKindPayload {
    fn from(source_kind: RenderMessageSourceKind) -> Self {
        match source_kind {
            RenderMessageSourceKind::Markdown => Self::Markdown,
            RenderMessageSourceKind::Stream => Self::Stream,
        }
    }
}

impl From<&crate::surface::cell_diff_data::AgentRenderDiffCell> for DiffCellPayload {
    fn from(cell: &crate::surface::cell_diff_data::AgentRenderDiffCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            changes: cell.changes.clone(),
            diagnostics: cell.diagnostics.clone(),
            text_writes_applied: cell.text_writes_applied,
            renames_applied: cell.renames_applied,
            truncated: cell.truncated,
            truncation_boundary: cell.truncation_boundary.clone(),
        }
    }
}

impl From<&crate::surface::cell_plan_data::AgentRenderPlanCell> for PlanCellPayload {
    fn from(cell: &crate::surface::cell_plan_data::AgentRenderPlanCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            explanation: cell.explanation.clone(),
            steps: cell.steps.iter().map(PlanStepPayload::from).collect(),
            source: cell.source.clone(),
            source_kind: PlanSourceKindPayload::from(cell.source_kind),
        }
    }
}

impl From<&crate::surface::cell_plan_data::AgentRenderPlanStep> for PlanStepPayload {
    fn from(step: &crate::surface::cell_plan_data::AgentRenderPlanStep) -> Self {
        Self {
            status: PlanStatusPayload::from(step.status),
            summary: step.summary.clone(),
        }
    }
}

impl From<crate::surface::cell_plan_data::AgentRenderPlanSourceKind> for PlanSourceKindPayload {
    fn from(source_kind: crate::surface::cell_plan_data::AgentRenderPlanSourceKind) -> Self {
        match source_kind {
            crate::surface::cell_plan_data::AgentRenderPlanSourceKind::Markdown => Self::Markdown,
            cell_plan_data::AgentRenderPlanSourceKind::Stream => Self::Stream,
        }
    }
}

impl From<crate::surface::cell_plan_data::AgentRenderPlanStatus> for PlanStatusPayload {
    fn from(status: crate::surface::cell_plan_data::AgentRenderPlanStatus) -> Self {
        match status {
            crate::surface::cell_plan_data::AgentRenderPlanStatus::Pending => Self::Pending,
            cell_plan_data::AgentRenderPlanStatus::Running => Self::Running,
            cell_plan_data::AgentRenderPlanStatus::Completed => Self::Completed,
            cell_plan_data::AgentRenderPlanStatus::Failed => Self::Failed,
            cell_plan_data::AgentRenderPlanStatus::Skipped => Self::Skipped,
        }
    }
}

impl From<&crate::surface::cell_review_data::AgentRenderReviewCell> for ReviewCellPayload {
    fn from(cell: &crate::surface::cell_review_data::AgentRenderReviewCell) -> Self {
        Self {
            id: cell.id.clone(),
            subject: cell.subject.clone(),
            status: cell.status.map(ReviewStatusPayload::from),
            details: cell.details.iter().map(ReviewDetailPayload::from).collect(),
        }
    }
}

impl From<&crate::surface::cell_review_data::AgentRenderReviewDetail> for ReviewDetailPayload {
    fn from(detail: &crate::surface::cell_review_data::AgentRenderReviewDetail) -> Self {
        Self {
            kind: ReviewDetailKindPayload::from(detail.kind),
            text: detail.text.clone(),
        }
    }
}

impl From<crate::surface::cell_review_data::AgentRenderReviewDetailKind>
    for ReviewDetailKindPayload
{
    fn from(kind: crate::surface::cell_review_data::AgentRenderReviewDetailKind) -> Self {
        match kind {
            crate::surface::cell_review_data::AgentRenderReviewDetailKind::Command => Self::Command,
            cell_review_data::AgentRenderReviewDetailKind::Diagnostic => Self::Diagnostic,
            crate::surface::cell_review_data::AgentRenderReviewDetailKind::Delta => Self::Delta,
            cell_review_data::AgentRenderReviewDetailKind::Message => Self::Message,
        }
    }
}

impl From<crate::surface::cell_review_data::AgentRenderReviewStatus> for ReviewStatusPayload {
    fn from(status: crate::surface::cell_review_data::AgentRenderReviewStatus) -> Self {
        match status {
            crate::surface::cell_review_data::AgentRenderReviewStatus::Approved => Self::Approved,
            cell_review_data::AgentRenderReviewStatus::Denied => Self::Denied,
            cell_review_data::AgentRenderReviewStatus::Pending => Self::Pending,
            cell_review_data::AgentRenderReviewStatus::Running => Self::Running,
            cell_review_data::AgentRenderReviewStatus::Completed => Self::Completed,
            cell_review_data::AgentRenderReviewStatus::Failed => Self::Failed,
            cell_review_data::AgentRenderReviewStatus::Blocked => Self::Blocked,
            cell_review_data::AgentRenderReviewStatus::Stopped => Self::Stopped,
            cell_review_data::AgentRenderReviewStatus::Unknown => Self::Unknown,
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderErrorCell> for ErrorCellPayload {
    fn from(cell: &crate::surface::cell_event_data::AgentRenderErrorCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            message: cell.message.clone(),
            details: cell.details.clone(),
            next_step: cell.next_step.clone(),
            severity: cell.severity.as_code().to_owned(),
            action: cell.action.as_code().to_owned(),
            cell_kind: cell.cell_kind.as_code().to_owned(),
        }
    }
}

impl From<&RenderStatusCell> for StatusCellPayload {
    fn from(cell: &RenderStatusCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            items: cell.items.iter().map(StatusItemPayload::from).collect(),
        }
    }
}

impl From<&RenderCommandCell> for CommandCellPayload {
    fn from(cell: &RenderCommandCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            items: cell.items.iter().map(StatusItemPayload::from).collect(),
        }
    }
}

impl From<&RenderStatusItem> for StatusItemPayload {
    fn from(item: &RenderStatusItem) -> Self {
        Self {
            kind: item.kind.clone(),
            summary: item.summary.clone(),
            phase: item.phase.map(LifecyclePhasePayload::from),
            indicator: item.indicator.clone(),
            tool_name: item.tool_name.clone(),
            duration_ms: item.duration_ms,
            turn_id: item.turn_id,
            attempt_index: item.attempt_index,
        }
    }
}

impl From<&RenderGenerationStatusCell> for GenerationStatusCellPayload {
    fn from(cell: &RenderGenerationStatusCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            items: cell.items.iter().map(LifecycleItemPayload::from).collect(),
        }
    }
}

impl From<&RenderReasoningCell> for ReasoningCellPayload {
    fn from(cell: &RenderReasoningCell) -> Self {
        Self {
            id: cell.id.clone(),
            text: cell.text.clone(),
            done: cell.done,
            elapsed_ms: cell.elapsed_ms,
        }
    }
}

impl From<&crate::surface::cell_turn_summary_data::AgentRenderTurnSummaryCell>
    for TurnSummaryCellPayload
{
    fn from(cell: &crate::surface::cell_turn_summary_data::AgentRenderTurnSummaryCell) -> Self {
        Self {
            id: cell.id.clone(),
            duration_ms: cell.duration_ms,
            total_tokens: cell.total_tokens,
            cache_tokens: cell.usage.map(crate::AgentTuiTokenUsage::cache_tokens),
            input_tokens: cell.usage.map(|usage| usage.input_tokens),
            cached_input_tokens: cell.usage.map(|usage| usage.cache_read_input_tokens),
            output_tokens: cell.usage.map(|usage| usage.output_tokens),
            reasoning_output_tokens: cell.usage.map(|usage| usage.reasoning_output_tokens),
            cache_hit_rate_basis_points: cell
                .usage
                .map(crate::AgentTuiTokenUsage::cache_hit_rate_basis_points),
            first_token_avg_ms: cell.first_token_avg_ms,
            generation_tps_x10: cell.generation_tps_x10,
        }
    }
}

impl From<&RenderLifecycleItem> for LifecycleItemPayload {
    fn from(item: &RenderLifecycleItem) -> Self {
        Self {
            id: item.id.clone(),
            kind: item.kind.clone(),
            summary: item.summary.clone(),
            phase: LifecyclePhasePayload::from(item.phase),
            indicator: item.indicator.clone(),
            tool_name: item.tool_name.clone(),
            duration_ms: item.duration_ms,
            attempt_index: item.attempt_index,
        }
    }
}

impl From<AgentTuiLifecyclePhase> for LifecyclePhasePayload {
    fn from(phase: AgentTuiLifecyclePhase) -> Self {
        match phase {
            AgentTuiLifecyclePhase::Streaming => Self::Streaming,
            AgentTuiLifecyclePhase::Generation => Self::Generation,
            AgentTuiLifecyclePhase::Notice => Self::Notice,
            AgentTuiLifecyclePhase::Command => Self::Command,
            AgentTuiLifecyclePhase::Unknown => Self::Unknown,
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderExecCell> for ExecCellPayload {
    fn from(cell: &crate::surface::cell_event_data::AgentRenderExecCell) -> Self {
        Self {
            id: cell.id.clone(),
            tool_name: cell.tool_name.clone(),
            display_label: cell.display_label.clone(),
            display_target: cell.display_target.clone(),
            raw_tool_name: cell.raw_tool_name.clone(),
            display_detail: cell.display_detail.clone(),
            status: ExecStatusPayload::from(cell.status),
            provider_call_id: cell.provider_call_id.clone(),
            arguments_preview: cell.arguments_preview.clone(),
            output_preview: cell.output_preview.clone(),
            summary: cell.summary.clone(),
            duration_ms: cell.duration_ms,
        }
    }
}

impl From<crate::surface::cell_event_data::AgentRenderExecStatus> for ExecStatusPayload {
    fn from(status: crate::surface::cell_event_data::AgentRenderExecStatus) -> Self {
        match status {
            crate::surface::cell_event_data::AgentRenderExecStatus::Running => Self::Running,
            cell_event_data::AgentRenderExecStatus::Completed => Self::Completed,
            cell_event_data::AgentRenderExecStatus::Failed => Self::Failed,
            crate::surface::cell_event_data::AgentRenderExecStatus::Output => Self::Output,
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderUsageCell> for UsageCellPayload {
    fn from(cell: &crate::surface::cell_event_data::AgentRenderUsageCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            total_tokens: cell.total_tokens,
            total: cell.total.clone(),
            input_tokens: cell.input_tokens,
            input: cell.input.clone(),
            cache_read_input_tokens: cell.cache_read_input_tokens,
            cache_read: cell.cache_read.clone(),
            cache_write_input_tokens: cell.cache_write_input_tokens,
            cache_write: cell.cache_write.clone(),
            output_tokens: cell.output_tokens,
            output: cell.output.clone(),
            reasoning_output_tokens: cell.reasoning_output_tokens,
            reasoning: cell.reasoning.clone(),
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderHookCell> for HookCellPayload {
    fn from(cell: &crate::surface::cell_event_data::AgentRenderHookCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            items: cell.items.iter().map(HookItemPayload::from).collect(),
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderHookItem> for HookItemPayload {
    fn from(item: &crate::surface::cell_event_data::AgentRenderHookItem) -> Self {
        Self {
            run_id: item.run_id.clone(),
            name: item.name.clone(),
            status: LifecycleStatusPayload::from(item.status),
            indicator: item.indicator.clone(),
            status_message: item.status_message.clone(),
            entries: item
                .entries
                .iter()
                .map(HookOutputEntryPayload::from)
                .collect(),
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderHookOutputEntry> for HookOutputEntryPayload {
    fn from(entry: &crate::surface::cell_event_data::AgentRenderHookOutputEntry) -> Self {
        Self {
            kind: HookOutputKindPayload::from(entry.kind),
            text: entry.text.clone(),
        }
    }
}

impl From<crate::surface::cell_event_data::AgentRenderHookOutputKind> for HookOutputKindPayload {
    fn from(kind: crate::surface::cell_event_data::AgentRenderHookOutputKind) -> Self {
        match kind {
            crate::surface::cell_event_data::AgentRenderHookOutputKind::Feedback => Self::Feedback,
            cell_event_data::AgentRenderHookOutputKind::Warning => Self::Warning,
            cell_event_data::AgentRenderHookOutputKind::Stop => Self::Stop,
            cell_event_data::AgentRenderHookOutputKind::Context => Self::Context,
            cell_event_data::AgentRenderHookOutputKind::Error => Self::Error,
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderMcpCell> for McpCellPayload {
    fn from(cell: &crate::surface::cell_event_data::AgentRenderMcpCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            empty_text: cell.empty_text.clone(),
            source: McpCellSourcePayload::from(cell.source),
            items: cell.items.iter().map(McpItemPayload::from).collect(),
        }
    }
}

impl From<crate::surface::cell_event_data::AgentRenderMcpCellSource> for McpCellSourcePayload {
    fn from(source: crate::surface::cell_event_data::AgentRenderMcpCellSource) -> Self {
        match source {
            crate::surface::cell_event_data::AgentRenderMcpCellSource::ToolCall => Self::ToolCall,
            cell_event_data::AgentRenderMcpCellSource::Catalog => Self::Catalog,
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderMcpItem> for McpItemPayload {
    fn from(item: &crate::surface::cell_event_data::AgentRenderMcpItem) -> Self {
        Self {
            provider_call_id: item.provider_call_id.clone(),
            server: item.server.clone(),
            tool: item.tool.clone(),
            display_label: item.display_label.clone(),
            display_target: item.display_target.clone(),
            raw_tool_name: item.raw_tool_name.clone(),
            display_detail: item.display_detail.clone(),
            status: LifecycleStatusPayload::from(item.status),
            arguments_preview: item.arguments_preview.clone(),
            output_preview: item.output_preview.clone(),
            summary: item.summary.clone(),
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderHistoryCell> for HistoryCellPayload {
    fn from(cell: &crate::surface::cell_event_data::AgentRenderHistoryCell) -> Self {
        Self {
            id: cell.id.clone(),
            title: cell.title.clone(),
            empty_text: cell.empty_text.clone(),
            items: cell.items.iter().map(HistoryItemPayload::from).collect(),
        }
    }
}

impl From<&crate::surface::cell_event_data::AgentRenderHistoryItem> for HistoryItemPayload {
    fn from(item: &crate::surface::cell_event_data::AgentRenderHistoryItem) -> Self {
        Self {
            name: item.name.clone(),
            kind_label: item.kind_label.clone(),
            status_label: item.status_label.clone(),
        }
    }
}

impl From<crate::surface::cell_event_data::AgentRenderLifecycleStatus> for LifecycleStatusPayload {
    fn from(status: crate::surface::cell_event_data::AgentRenderLifecycleStatus) -> Self {
        match status {
            crate::surface::cell_event_data::AgentRenderLifecycleStatus::Completed => {
                Self::Completed
            }
            cell_event_data::AgentRenderLifecycleStatus::Failed => Self::Failed,
            cell_event_data::AgentRenderLifecycleStatus::Running => Self::Running,
            cell_event_data::AgentRenderLifecycleStatus::Pending => Self::Pending,
            cell_event_data::AgentRenderLifecycleStatus::Blocked => Self::Blocked,
            cell_event_data::AgentRenderLifecycleStatus::Stopped => Self::Stopped,
            cell_event_data::AgentRenderLifecycleStatus::Unknown => Self::Unknown,
        }
    }
}

#[cfg(test)]
#[path = "projection_tests.rs"]
mod projection_tests;
