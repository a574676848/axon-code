//! AgentContext Memory recall 摘要。

use axon_domain::MemorySourceOrigin;
use axon_memory::MemoryHybridHit;

use crate::agent_context_memory_layers::{
    classify_long_term_origin, code_symbol_layer, memory_recall_layer_label,
};
use crate::{MemoryCodeSymbolHit, MemoryCodeSymbolKind, agent_context::MemoryWorkingRecallItem};

/// 将已排序的长期记忆命中投影为 provider 可见的有界摘要。
pub fn memory_recall_summary(hits: &[MemoryHybridHit]) -> Option<String> {
    memory_recall_summary_from_parts(&[], hits, &[])
}

pub(super) fn memory_recall_summary_from_parts(
    working: &[MemoryWorkingRecallItem],
    hits: &[MemoryHybridHit],
    code_symbol_hits: &[MemoryCodeSymbolHit],
) -> Option<String> {
    let mut lines = working_memory_recall_lines(working);
    lines.extend(hits.iter().filter_map(memory_recall_line));
    lines.extend(code_symbol_hits.iter().filter_map(code_symbol_recall_line));
    (!lines.is_empty()).then(|| lines.join("\n"))
}

fn working_memory_recall_lines(working: &[MemoryWorkingRecallItem]) -> Vec<String> {
    working
        .iter()
        .map(|item| {
            format!(
                "layer={} label={} source_ref={} token_estimate={} persisted=false",
                memory_recall_layer_label(
                    crate::agent_context_memory_layers::MemoryRecallLayer::Working
                ),
                item.label,
                item.source_ref,
                item.token_estimate
            )
        })
        .collect()
}

fn memory_recall_line(hit: &MemoryHybridHit) -> Option<String> {
    let content_ref = hit.record.record.content_ref.as_deref()?.trim();
    if content_ref.is_empty() {
        return None;
    }

    let source_trace = hit.record.record.source_trace.as_ref();
    let layer = classify_long_term_origin(source_trace.map(|trace| trace.origin));
    let source_origin =
        source_trace.map_or("unknown", |trace| memory_source_origin_label(trace.origin));
    let source_session = source_trace
        .and_then(|trace| trace.session_id)
        .map_or_else(|| "none".to_owned(), |session_id| session_id.0.to_string());
    let source_cell = source_trace
        .and_then(|trace| trace.cell_id)
        .map_or_else(|| "none".to_owned(), |cell_id| cell_id.0.to_string());
    let memory_summary = source_trace
        .map(|trace| trace.summary.trim())
        .filter(|summary| !summary.is_empty())
        .unwrap_or("none");
    let evidence_ref = source_trace
        .and_then(|trace| trace.evidence_ref.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("none");
    let vector_distance = hit
        .vector_distance
        .map_or_else(|| "none".to_owned(), |distance| format!("{distance:.4}"));

    Some(format!(
        "layer={} memory_id={} state={} content_ref={} score_bp={} text_rank={} vector_rank={} vector_distance={} confidence_bp={} importance_bp={} source_origin={} source_session={} source_cell={} memory_summary={} evidence_ref={}",
        memory_recall_layer_label(layer),
        hit.record.record.id.0,
        memory_state_label(hit.record.record.state),
        content_ref,
        hit.score_basis_points,
        optional_rank(hit.text_rank),
        optional_rank(hit.vector_rank),
        vector_distance,
        hit.record.confidence_basis_points,
        hit.record.importance_basis_points,
        source_origin,
        source_session,
        source_cell,
        memory_summary,
        evidence_ref
    ))
}

fn code_symbol_recall_line(hit: &MemoryCodeSymbolHit) -> Option<String> {
    if !hit.ref_only {
        return None;
    }
    let symbol_ref = trimmed_or_none(&hit.symbol_ref)?;
    let symbol_name = trimmed_or_none(&hit.symbol_name)?;
    let path_ref = trimmed_or_none(&hit.path_ref)?;
    let source_hash = trimmed_or_none(&hit.source_summary_hash)?;
    Some(format!(
        "layer={} code_symbol_ref={} symbol_name={} kind={} path_ref={} source_hash={} score_bp={} ref_only=true",
        memory_recall_layer_label(code_symbol_layer(hit)),
        symbol_ref,
        symbol_name,
        code_symbol_kind_label(hit.kind),
        path_ref,
        source_hash,
        hit.score_basis_points
    ))
}

const fn memory_source_origin_label(origin: MemorySourceOrigin) -> &'static str {
    match origin {
        MemorySourceOrigin::LocalTimeline => "local_timeline",
        MemorySourceOrigin::AgentUserContext => "agent_user_context",
        MemorySourceOrigin::AgentExperience => "agent_experience",
        MemorySourceOrigin::AgentKnowledge => "agent_knowledge",
        MemorySourceOrigin::AgentProcedure => "agent_procedure",
        MemorySourceOrigin::ExplicitUserPreference => "explicit_user_preference",
        MemorySourceOrigin::DebuggingExperience => "debugging_experience",
        MemorySourceOrigin::RemoteUpload => "remote_upload",
        MemorySourceOrigin::CommandCompletionFeedback => "command_completion_feedback",
        MemorySourceOrigin::SkillReflection => "skill_reflection",
    }
}

const fn memory_state_label(state: axon_domain::MemoryState) -> &'static str {
    match state {
        axon_domain::MemoryState::Weak => "weak",
        axon_domain::MemoryState::Strong => "strong",
        axon_domain::MemoryState::Quarantined => "quarantined",
        axon_domain::MemoryState::Tombstone => "tombstone",
    }
}

fn optional_rank(rank: Option<usize>) -> String {
    rank.map_or_else(|| "none".to_owned(), |rank| rank.to_string())
}

fn trimmed_or_none(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then_some(trimmed)
}

const fn code_symbol_kind_label(kind: MemoryCodeSymbolKind) -> &'static str {
    match kind {
        MemoryCodeSymbolKind::Function => "function",
        MemoryCodeSymbolKind::Type => "type",
        MemoryCodeSymbolKind::Module => "module",
        MemoryCodeSymbolKind::EntryPoint => "entry_point",
    }
}
