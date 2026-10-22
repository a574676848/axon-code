#[path = "tool_output_preview.rs"]
mod tool_output_preview;

use axon_display::normalize_display_units_in_text;

pub(crate) use tool_output_preview::executed_command_preview as shared_executed_command_preview;
#[cfg(test)]
pub(super) use tool_output_preview::{REDACTED_VALUE, TOOL_OUTPUT_CHUNK_MAX_CHARS};
pub(in crate::r#loop::agent_tui) use tool_output_preview::{
    compact_tool_output_chunk, executed_command_preview, tool_arguments_preview,
};

use super::run_output::{
    AgentTuiToolExecutionView, AgentTuiToolKind, AgentTuiToolLifecycleLine,
    AgentTuiToolOutputChunk, AgentTuiToolProgressView, AgentTuiToolReceipt,
};
use crate::ToolDisplayProjection;
use crate::surface::{
    cell::{RenderCellAction, RenderCellRecord},
    cell_event_data::{AgentRenderExecCell, AgentRenderExecCompletion, AgentRenderMcpCell},
};

const MIN_VISIBLE_PROGRESS_ELAPSED_MS: u64 = 1;
const MACHINE_SUMMARY_MARKERS: &[&str] = &[
    " command=",
    " exit_code=",
    " timed_out=",
    " output_handler=",
    " full_output_path=",
    " stdout=",
    " stderr=",
];

const TOOL_ARGUMENTS_PREVIEW_KEY: &str = "argumentsPreview";
const TOOL_QUERY_KEY: &str = "query";
const TOOL_PATH_KEY: &str = "path";
const TOOL_ROOT_KEY: &str = "root";
const TOOL_AFFECTED_PATHS_KEY: &str = "affectedPaths";
const TOOL_PATHS_KEY: &str = "paths";
const TERMINAL_INTERACTION_KEY: &str = "terminalInteraction";
const TERMINAL_INTERACTION_STDIN_KEY: &str = "stdin";

pub(super) fn tool_execution_started_line(view: &AgentTuiToolExecutionView) -> RenderCellRecord {
    let projection = ToolDisplayProjection::from_tool_call(
        &view.tool_name,
        view.semantic_kind,
        &view.arguments_preview,
        view.intent.as_deref(),
    );
    if view.semantic_kind == AgentTuiToolKind::Mcp {
        return RenderCellRecord::mcp_with_data(AgentRenderMcpCell::from_tool_execution(view));
    }
    let mut cell = AgentRenderExecCell::running_with_id(
        view.part_id.clone(),
        view.tool_name.clone(),
        view.provider_call_id.clone(),
        Some(tool_arguments_preview(&view.arguments_preview)),
    );
    cell.set_display_projection(projection);
    RenderCellRecord::exec_with_data(
        cell,
        terminal_tool_action(tool_kind_action(view.semantic_kind), "running"),
    )
}

pub(super) fn tool_result_line(receipt: &AgentTuiToolReceipt) -> RenderCellRecord {
    let preview = ToolReceiptPreview::from_receipt(receipt);
    let action = tool_receipt_action(receipt);
    if receipt.kind == AgentTuiToolKind::Mcp {
        return RenderCellRecord::mcp_with_data(AgentRenderMcpCell::from_tool_receipt(
            receipt,
            preview.summary_preview,
            preview.arguments_preview,
            preview.output_preview,
        ));
    }
    let projection = ToolDisplayProjection::from_tool(
        &receipt.tool_name,
        receipt.kind,
        receipt.metadata.as_ref(),
    );
    let mut cell = AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
        id: receipt.id.clone(),
        tool_name: receipt.tool_name.clone(),
        status: receipt.status.clone(),
        provider_call_id: receipt.provider_call_id.clone(),
        summary: preview.summary_preview,
        arguments_preview: preview.arguments_preview,
        output_preview: preview.output_preview,
        duration_ms: None,
    });
    cell.set_display_projection(projection);
    RenderCellRecord::exec_with_data(cell, action)
}

pub(super) fn merge_tool_result_text(current: &str, receipt: &AgentTuiToolReceipt) -> String {
    let current = current.trim();
    if current.is_empty() {
        ToolDisplayProjection::from_tool(
            &receipt.tool_name,
            receipt.kind,
            receipt.metadata.as_ref(),
        )
        .label
    } else {
        current.to_owned()
    }
}

pub(super) fn merge_tool_output_chunk_text(
    current: &str,
    chunk: &AgentTuiToolOutputChunk,
) -> String {
    let current = current.trim();
    if current.is_empty() {
        ToolDisplayProjection::from_tool(&chunk.tool_name, chunk.semantic_kind, None).label
    } else {
        current.to_owned()
    }
}

pub(super) fn tool_output_chunk_line(chunk: &AgentTuiToolOutputChunk) -> RenderCellRecord {
    let projection = ToolDisplayProjection::from_tool(&chunk.tool_name, chunk.semantic_kind, None);
    let mut cell = AgentRenderExecCell::output_with_id(
        chunk.part_id.clone(),
        chunk.tool_name.clone(),
        chunk.provider_call_id.clone(),
        Some(tool_output_chunk_preview(chunk)),
    );
    cell.set_display_projection(projection);
    RenderCellRecord::exec_with_data(
        cell,
        terminal_tool_action(tool_kind_action(chunk.semantic_kind), "output"),
    )
}

pub(super) fn tool_output_chunk_preview(chunk: &AgentTuiToolOutputChunk) -> String {
    compact_tool_output_chunk(&chunk.text)
}

pub(super) fn tool_progress_line(progress: &AgentTuiToolProgressView) -> RenderCellRecord {
    let projection =
        ToolDisplayProjection::from_tool(&progress.tool_name, progress.semantic_kind, None);
    let mut cell = AgentRenderExecCell::output_with_id(
        progress.part_id.clone(),
        progress.tool_name.clone(),
        progress.provider_call_id.clone(),
        Some(tool_progress_preview(progress)),
    );
    cell.set_display_projection(projection);
    RenderCellRecord::exec_with_data(
        cell,
        terminal_tool_action(tool_kind_action(progress.semantic_kind), "progress"),
    )
}

pub(super) fn tool_progress_preview(progress: &AgentTuiToolProgressView) -> String {
    let mut parts = Vec::new();
    let phase = progress.phase.trim();
    if !phase.is_empty() {
        parts.push(phase.to_owned());
    }
    if let Some(items) = progress_items_text(progress.items_completed, progress.items_total) {
        parts.push(items);
    }
    if let Some(current_item) = progress
        .current_item
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(format!("当前 {current_item}"));
    }
    if let Some(message) = progress
        .message
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        parts.push(message.to_owned());
    }
    if let Some(elapsed_ms) = progress
        .elapsed_ms
        .filter(|elapsed_ms| *elapsed_ms >= MIN_VISIBLE_PROGRESS_ELAPSED_MS)
    {
        parts.push(crate::r#loop::agent_tui::duration_format::format_elapsed(
            std::time::Duration::from_millis(elapsed_ms),
        ));
    }
    if parts.is_empty() {
        return ToolDisplayProjection::from_tool(&progress.tool_name, progress.semantic_kind, None)
            .label;
    }
    parts.join(" · ")
}

fn progress_items_text(completed: Option<u64>, total: Option<u64>) -> Option<String> {
    match (completed, total) {
        (Some(completed), Some(total)) => Some(format!("{completed}/{total}")),
        (Some(completed), None) => Some(format!("{completed} completed")),
        (None, Some(total)) => Some(format!("total {total}")),
        (None, None) => None,
    }
}

pub(super) fn tool_lifecycle_line(line: &AgentTuiToolLifecycleLine) -> RenderCellRecord {
    let projection = ToolDisplayProjection::from_tool(&line.tool_name, line.kind, None);
    let mut cell = AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
        id: line.tool_part_id.clone(),
        tool_name: line.tool_name.clone(),
        status: line.status.clone(),
        provider_call_id: line.provider_call_id.clone(),
        summary: tool_lifecycle_summary_preview(line),
        arguments_preview: line
            .arguments_preview
            .as_deref()
            .map(executed_command_preview),
        output_preview: line.output_preview.clone(),
        duration_ms: None,
    });
    cell.set_display_projection(projection);
    RenderCellRecord::exec_with_data(
        cell,
        terminal_tool_action(tool_kind_action(line.kind), &line.status),
    )
}

fn tool_lifecycle_summary_preview(line: &AgentTuiToolLifecycleLine) -> Option<String> {
    if tool_receipt_status_is_quiet_success(&line.status) {
        return None;
    }
    plain_summary_preview(&line.summary)
}

#[derive(Debug, Clone, Default)]
struct ToolReceiptPreview {
    summary_preview: Option<String>,
    arguments_preview: Option<String>,
    output_preview: Option<String>,
}

impl ToolReceiptPreview {
    fn from_receipt(receipt: &AgentTuiToolReceipt) -> Self {
        let Some(metadata) = receipt.metadata.as_ref() else {
            return Self {
                summary_preview: receipt_summary_preview(receipt),
                ..Self::default()
            };
        };
        Self {
            summary_preview: summary_preview(receipt, metadata),
            arguments_preview: receipt_arguments_preview(receipt, metadata)
                .map(|value| executed_command_preview(&value)),
            output_preview: preview_display_field(metadata, "outputPreview"),
        }
    }
}

fn receipt_arguments_preview(
    receipt: &AgentTuiToolReceipt,
    metadata: &serde_json::Value,
) -> Option<String> {
    if is_write_stdin_tool(&receipt.tool_name) {
        return metadata
            .get(TERMINAL_INTERACTION_KEY)
            .and_then(|value| preview_field(value, TERMINAL_INTERACTION_STDIN_KEY));
    }
    preview_field(metadata, TOOL_ARGUMENTS_PREVIEW_KEY).or_else(|| match receipt.kind {
        AgentTuiToolKind::Search | AgentTuiToolKind::MemorySearch | AgentTuiToolKind::History => {
            preview_field(metadata, TOOL_QUERY_KEY)
        }
        AgentTuiToolKind::ReadFile | AgentTuiToolKind::MemoryWrite => {
            preview_field(metadata, TOOL_PATH_KEY)
        }
        AgentTuiToolKind::Notes => preview_field(metadata, TOOL_QUERY_KEY)
            .or_else(|| preview_field(metadata, TOOL_PATH_KEY)),
        AgentTuiToolKind::Glob => preview_field(metadata, TOOL_ROOT_KEY),
        AgentTuiToolKind::Edit => first_metadata_array_string(metadata, TOOL_PATHS_KEY)
            .or_else(|| first_metadata_array_string(metadata, TOOL_AFFECTED_PATHS_KEY)),
        AgentTuiToolKind::Unsupported => first_metadata_array_string(metadata, TOOL_PATHS_KEY),
        _ => None,
    })
}

fn is_write_stdin_tool(tool_name: &str) -> bool {
    tool_name
        .rsplit(['.', ':', '/'])
        .next()
        .is_some_and(|name| name == "write_stdin")
}

fn first_metadata_array_string(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_array)
        .and_then(|values| values.iter().find_map(serde_json::Value::as_str))
        .map(ToOwned::to_owned)
        .filter(|value| !value.trim().is_empty())
}

fn summary_preview(receipt: &AgentTuiToolReceipt, metadata: &serde_json::Value) -> Option<String> {
    if tool_receipt_status_is_quiet_success(&receipt.status) {
        return None;
    }
    preview_display_field(metadata, "summary")
        .and_then(|value| plain_summary_preview(&value))
        .or_else(|| receipt_summary_preview(receipt))
}

fn tool_receipt_status_is_quiet_success(status: &str) -> bool {
    matches!(status, "completed" | "success")
}

fn receipt_summary_preview(receipt: &AgentTuiToolReceipt) -> Option<String> {
    if tool_receipt_status_is_quiet_success(&receipt.status) {
        return None;
    }
    plain_summary_preview(&receipt.summary)
}

fn plain_summary_preview(summary: &str) -> Option<String> {
    let trimmed = summary.trim();
    if trimmed.is_empty() || looks_like_machine_tool_summary(trimmed) {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn looks_like_machine_tool_summary(summary: &str) -> bool {
    let marker_count = MACHINE_SUMMARY_MARKERS
        .iter()
        .filter(|marker| summary.contains(**marker))
        .count();
    marker_count >= 2
}

fn preview_field(metadata: &serde_json::Value, key: &str) -> Option<String> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.trim().is_empty())
}

fn preview_display_field(metadata: &serde_json::Value, key: &str) -> Option<String> {
    preview_field(metadata, key).map(|value| normalize_display_units_in_text(&value))
}

fn tool_receipt_action(receipt: &AgentTuiToolReceipt) -> RenderCellAction {
    terminal_tool_action(tool_kind_action(receipt.kind), &receipt.status)
}

pub(super) const fn tool_kind_action(kind: AgentTuiToolKind) -> RenderCellAction {
    match kind {
        AgentTuiToolKind::Search | AgentTuiToolKind::MemorySearch | AgentTuiToolKind::History => {
            RenderCellAction::Search
        }
        AgentTuiToolKind::ReadFile => RenderCellAction::Read,
        AgentTuiToolKind::Glob => RenderCellAction::List,
        AgentTuiToolKind::Mcp => RenderCellAction::Mcp,
        AgentTuiToolKind::MemoryWrite | AgentTuiToolKind::Notes | AgentTuiToolKind::Edit => {
            RenderCellAction::Patched
        }
        AgentTuiToolKind::InvokeSkill | AgentTuiToolKind::Shell | AgentTuiToolKind::Unsupported => {
            RenderCellAction::Run
        }
    }
}

fn terminal_tool_action(action: RenderCellAction, status: &str) -> RenderCellAction {
    match status {
        "pending" | "queued" => RenderCellAction::Waiting,
        "running" | "calling" | "output" => RenderCellAction::Running,
        "completed" | "success" => match action {
            RenderCellAction::Run | RenderCellAction::Running => RenderCellAction::Ran,
            RenderCellAction::Search => RenderCellAction::Searched,
            other => other,
        },
        "failed" | "error" => RenderCellAction::Error,
        _ => action,
    }
}

#[cfg(test)]
#[path = "tool_output_projection_tests.rs"]
mod tool_output_projection_tests;
#[cfg(test)]
#[path = "tool_output_resume_tests.rs"]
mod tool_output_resume_tests;
