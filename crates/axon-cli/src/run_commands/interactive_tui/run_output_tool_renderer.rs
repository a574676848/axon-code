use std::collections::{BTreeMap, BTreeSet};

use axon_protocol::wire::{
    WireEventEnvelope, WireEventKind, WireLifecycleStatus, WireToolCall, WireToolResult,
    WireUsageUpdate,
};
use axon_tui_runtime::{
    AgentTuiTokenUsage, AgentTuiToolKind, AgentTuiToolReceipt, AgentTuiToolReceiptDetails,
    AgentTuiUsageLine,
};

/// durable 有序投影使用的工具回执行过滤器。
///
/// 文件变更已经独立投影时，对应的工具回执不再重复成 cell。
#[derive(Debug, Default)]
pub(super) struct WireToolReceiptFilter {
    calls: BTreeMap<String, WireToolCall>,
    file_change_tool_part_ids: BTreeSet<String>,
    file_change_tool_call_ids: BTreeSet<String>,
    has_file_changes: bool,
}

impl WireToolReceiptFilter {
    pub(super) fn build(events: &[WireEventEnvelope]) -> Self {
        let mut filter = Self::default();
        for event in events {
            match &event.kind {
                WireEventKind::ToolCall(call) => {
                    filter.calls.insert(call.tool_call_id.clone(), call.clone());
                }
                WireEventKind::FileChange(change) => {
                    filter.has_file_changes = true;
                    if let Some(part_id) = change
                        .tool_part_id
                        .as_deref()
                        .filter(|part_id| !part_id.trim().is_empty())
                    {
                        filter.file_change_tool_part_ids.insert(part_id.to_owned());
                    }
                    if let Some(call_id) = change
                        .tool_call_id
                        .as_deref()
                        .filter(|call_id| !call_id.trim().is_empty())
                    {
                        filter.file_change_tool_call_ids.insert(call_id.to_owned());
                    }
                }
                _ => {}
            }
        }
        filter
    }

    pub(super) fn receipt_for(&self, result: &WireToolResult) -> Option<AgentTuiToolReceipt> {
        if self
            .file_change_tool_part_ids
            .contains(result.part_id.as_str())
            || self
                .file_change_tool_call_ids
                .contains(result.tool_call_id.as_str())
        {
            return None;
        }
        if self.has_file_changes && result.filesystem_write_performed {
            return None;
        }
        Some(wire_tool_receipt(result, &self.calls))
    }
}

pub(super) fn latest_wire_usage(events: &[WireEventEnvelope]) -> Option<AgentTuiUsageLine> {
    events.iter().rev().find_map(|event| match &event.kind {
        WireEventKind::UsageUpdate(usage) => Some(wire_usage_line(event, *usage)),
        _ => None,
    })
}

/// 单帧 usage 快照投影；稳定 part id 由 durable event id 派生。
pub(super) fn wire_usage_line(
    event: &WireEventEnvelope,
    usage: WireUsageUpdate,
) -> AgentTuiUsageLine {
    AgentTuiUsageLine::with_id(
        format!("usage:wire:{}", event.event_id.0),
        wire_usage_to_agent_tui_usage(usage),
    )
}

fn wire_tool_receipt(
    result: &WireToolResult,
    calls: &BTreeMap<String, WireToolCall>,
) -> AgentTuiToolReceipt {
    let call = calls.get(&result.tool_call_id);
    let tool_name = call.map_or_else(
        || result.tool_call_id.clone(),
        |call| call.tool_name.clone(),
    );
    let kind = call
        .map(|call| AgentTuiToolKind::from_code(&call.tool_kind))
        .filter(|kind| *kind != AgentTuiToolKind::Unsupported)
        .unwrap_or_else(|| resolved_tool_kind(result.metadata.as_ref(), &tool_name));
    AgentTuiToolReceipt::with_id(
        result.part_id.clone(),
        result.tool_call_id.clone(),
        tool_name,
        AgentTuiToolReceiptDetails {
            kind,
            status: lifecycle_status_name(result.status).to_owned(),
            summary: result.summary.clone(),
            metadata: result.metadata.clone(),
            error_type: result.error_type.clone(),
            filesystem_write_performed: result.filesystem_write_performed,
        },
    )
}

pub(super) fn wire_usage_to_agent_tui_usage(usage: WireUsageUpdate) -> AgentTuiTokenUsage {
    AgentTuiTokenUsage::new(
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.cache_write_input_tokens,
        usage.output_tokens,
        usage.reasoning_output_tokens,
    )
    .with_current_context_tokens(usage.current_context_tokens)
}

pub(super) fn lifecycle_status_name(status: WireLifecycleStatus) -> &'static str {
    match status {
        WireLifecycleStatus::Started => "started",
        WireLifecycleStatus::Running => "running",
        WireLifecycleStatus::Waiting => "waiting",
        WireLifecycleStatus::Completed => "completed",
        WireLifecycleStatus::Failed => "failed",
        WireLifecycleStatus::Cancelled => "cancelled",
        WireLifecycleStatus::Aborted => "aborted",
        WireLifecycleStatus::Blocked => "blocked",
    }
}

fn tool_kind_from_metadata(metadata: Option<&serde_json::Value>) -> AgentTuiToolKind {
    let Some(metadata) = metadata else {
        return AgentTuiToolKind::Unsupported;
    };
    metadata
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .map_or(AgentTuiToolKind::Unsupported, AgentTuiToolKind::from_code)
}

fn resolved_tool_kind(metadata: Option<&serde_json::Value>, tool_name: &str) -> AgentTuiToolKind {
    let metadata_kind = tool_kind_from_metadata(metadata);
    if metadata_kind != AgentTuiToolKind::Unsupported {
        return metadata_kind;
    }
    tool_kind_from_name(tool_name)
}

fn tool_kind_from_name(tool_name: &str) -> AgentTuiToolKind {
    AgentTuiToolKind::from_tool_name(tool_name)
}
