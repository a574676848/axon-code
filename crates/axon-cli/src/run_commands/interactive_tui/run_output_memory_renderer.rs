use axon_protocol::wire::{WireMemoryStatus, memory_surface_text_is_user_visible};
use axon_tui_runtime::{AgentTuiHistoryItem, AgentTuiHistoryLine, AgentTuiRunLine};
use serde::Deserialize;

use super::run_output_renderer::bounded_single_line;

const MEMORY_RECALL_PREVIEW_LIMIT: usize = 3;
const MEMORY_RECALL_REF_MAX_CHARS: usize = 72;
const MEMORY_RECALL_SKIPPED_STATUS: &str = "skipped";
const MEMORY_RECALL_SUBJECT: &str = "recall";
const MEMORY_HIT_COUNT_KEY: &str = "hitCount";
const MEMORY_TRACE_KEY: &str = "trace";
const MEMORY_RECALL_HISTORY_PART_ID: &str = "history:memory:recall";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TuiMemoryRecallTraceEvent {
    content_ref: String,
    score_basis_points: u16,
    scope_match: String,
    #[serde(default)]
    prompt_injected: bool,
}

/// 单帧记忆检索事实投影；durable 顺序由调用方保证。
pub(super) fn memory_recall_line_for(status: &WireMemoryStatus) -> Option<AgentTuiRunLine> {
    if status.subject != MEMORY_RECALL_SUBJECT
        || status.outcome == MEMORY_RECALL_SKIPPED_STATUS
        || memory_hit_count(status) == 0
    {
        return None;
    }

    let items = memory_trace(status)
        .into_iter()
        .filter(|trace| trace.prompt_injected)
        .filter_map(|trace| memory_recall_item(&trace))
        .take(MEMORY_RECALL_PREVIEW_LIMIT)
        .collect::<Vec<_>>();

    (!items.is_empty()).then(|| {
        AgentTuiRunLine::History(AgentTuiHistoryLine::with_id(
            MEMORY_RECALL_HISTORY_PART_ID,
            "记忆",
            items,
        ))
    })
}

fn memory_hit_count(event: &WireMemoryStatus) -> u64 {
    event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(MEMORY_HIT_COUNT_KEY))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_else(|| u64::try_from(event.refs.len()).unwrap_or(u64::MAX))
}

fn memory_trace(event: &WireMemoryStatus) -> Vec<TuiMemoryRecallTraceEvent> {
    event
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get(MEMORY_TRACE_KEY))
        .cloned()
        .and_then(|trace| serde_json::from_value(trace).ok())
        .unwrap_or_default()
}

fn memory_recall_item(trace: &TuiMemoryRecallTraceEvent) -> Option<AgentTuiHistoryItem> {
    let content_ref = bounded_single_line(&trace.content_ref, MEMORY_RECALL_REF_MAX_CHARS);
    if !memory_surface_text_is_user_visible(&content_ref) {
        return None;
    }

    Some(AgentTuiHistoryItem::new(
        content_ref,
        Some("记忆".to_owned()),
        Some(memory_score_label(trace)),
    ))
}

fn memory_score_label(trace: &TuiMemoryRecallTraceEvent) -> String {
    let percent = u32::from(trace.score_basis_points.min(10_000)) / 100;
    let scope = trace.scope_match.trim();
    if scope.is_empty() {
        return format!("{percent}%");
    }
    format!("{percent}% · {scope}")
}
