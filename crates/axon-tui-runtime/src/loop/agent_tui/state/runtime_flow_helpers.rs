//! Runtime flow 纯投射辅助函数。

use std::time::Instant;

use axon_time::duration_ms;

use crate::r#loop::agent_tui::{
    AgentTuiConfigStatus, AgentTuiModelCatalogItem, AgentTuiRunOverrides,
    run_output::{AgentTuiLifecycleStatusView, AgentTuiLifecycleStatusViewInput, AgentTuiRunLine},
    runtime_identity::{effective_model_identity, provider_model_route},
    tool_output_projection::{
        merge_tool_output_chunk_text, tool_output_chunk_line, tool_output_chunk_preview,
    },
};
use crate::surface::cell::RenderCellRecord;

use super::runtime_flow_cell_access::{exec_cell_data, exec_cell_data_mut, mcp_cell_data_mut};
use super::runtime_projection_policy::{RuntimeProjectionPolicy, RuntimeProjectionTarget};

const STATUS_TITLE_GENERATION: &str = "生成";
const STATUS_PHASE_GENERATION: &str = "generation";
const STATUS_KIND_STREAMING_STARTED: &str = "StreamingStarted";
const STATUS_SUMMARY_REPLY_SUFFIX: &str = "生成回复中";
const RETRY_LIFECYCLE_KIND_PROVIDER_RETRY: &str = "ProviderRetry";
const RETRY_LIFECYCLE_KIND_PROVIDER_RETRY_SNAKE: &str = "provider_retry";
const FALLBACK_REPLY_LIFECYCLE_ID: &str = "lifecycle:fallback:reply";

pub(super) fn lifecycle_status_is_retry(kind: &str) -> bool {
    matches!(
        kind,
        RETRY_LIFECYCLE_KIND_PROVIDER_RETRY | RETRY_LIFECYCLE_KIND_PROVIDER_RETRY_SNAKE
    )
}

pub(super) fn fallback_reply_lifecycle_status(
    config_status: &AgentTuiConfigStatus,
    overrides: &AgentTuiRunOverrides,
    models: &[AgentTuiModelCatalogItem],
) -> AgentTuiLifecycleStatusView {
    let identity = effective_model_identity(config_status, overrides, models);
    AgentTuiLifecycleStatusView::with_id(AgentTuiLifecycleStatusViewInput {
        id: FALLBACK_REPLY_LIFECYCLE_ID.to_owned(),
        title: STATUS_TITLE_GENERATION.to_owned(),
        kind: STATUS_KIND_STREAMING_STARTED.to_owned(),
        summary: generating_reply_summary(&identity.provider, &identity.model),
        phase: STATUS_PHASE_GENERATION.to_owned(),
        tool_name: (!identity.model.trim().is_empty()).then(|| identity.model.clone()),
        duration_ms: None,
        attempt_index: None,
    })
}

fn generating_reply_summary(provider: &str, model: &str) -> String {
    let route = provider_model_route(provider, model);
    if route.is_empty() {
        STATUS_SUMMARY_REPLY_SUFFIX.to_owned()
    } else {
        format!("{route} {STATUS_SUMMARY_REPLY_SUFFIX}")
    }
}

pub(super) fn run_line_is_transient_final_output(line: &AgentTuiRunLine) -> bool {
    RuntimeProjectionPolicy::final_output_target(line) != RuntimeProjectionTarget::Transcript
}

pub(super) fn duration_since(started_at: Option<Instant>) -> Option<u64> {
    let elapsed = started_at?.elapsed();
    Some(duration_ms(elapsed))
}

pub(super) fn stamp_exec_duration(line: &mut RenderCellRecord, duration_ms: Option<u64>) {
    if let Some(cell) = exec_cell_data_mut(line) {
        cell.set_duration_ms(duration_ms);
    }
}

pub(super) fn merge_tool_output_chunk_into_cell(
    line: &mut RenderCellRecord,
    chunk: &crate::r#loop::agent_tui::run_output::AgentTuiToolOutputChunk,
) {
    line.text = merge_tool_output_chunk_text(&line.text, chunk);
    line.bump_render_revision();
    if let Some(cell) = exec_cell_data_mut(line) {
        let output_line = tool_output_chunk_line(chunk);
        if let Some(output_cell) = exec_cell_data(&output_line)
            && let Some(output_preview) = output_cell.output_preview.clone()
        {
            cell.merge_output_preview(output_preview);
        }
    }
    if let Some(cell) = mcp_cell_data_mut(line) {
        cell.merge_output_preview(tool_output_chunk_preview(chunk));
    }
}
