use axon_agent_runtime::MemorySqliteRecord;
use axon_domain::{MemorySourceOrigin, MemoryState};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryRecordView {
    pub(super) memory_id: u64,
    pub(super) candidate_id: u64,
    pub(super) memory_kind: &'static str,
    pub(super) state: &'static str,
    pub(super) content_ref: Option<String>,
    pub(super) evidence_ref: Option<String>,
    pub(super) source_origin: Option<&'static str>,
    pub(super) source_session_id: Option<u64>,
    pub(super) source_cell_id: Option<u64>,
    pub(super) memory_summary: Option<String>,
    pub(super) tombstone_blocked_ref: Option<String>,
    pub(super) has_tombstone_reason: bool,
    pub(super) namespace_ref: Option<String>,
    pub(super) repo_ref: Option<String>,
    pub(super) user_ref: Option<String>,
    pub(super) created_at_ms: u64,
    pub(super) updated_at_ms: u64,
    pub(super) last_active_at_ms: u64,
    pub(super) ttl_expires_at_ms: Option<u64>,
    pub(super) confidence_basis_points: u16,
    pub(super) importance_basis_points: u16,
    pub(super) archived_at_ms: Option<u64>,
    pub(super) vector_indexed: bool,
    pub(super) prompt_text_persisted: bool,
    pub(super) raw_payload_persisted: bool,
    pub(super) remote_write_allowed: bool,
}

pub(super) fn record_view(record: &MemorySqliteRecord) -> MemoryRecordView {
    let source_trace = record.record.source_trace.as_ref();
    let tombstone = record.record.tombstone.as_ref();
    MemoryRecordView {
        memory_id: record.record.id.0,
        candidate_id: record.record.candidate_id.0,
        memory_kind: record.memory_kind.as_str(),
        state: memory_state_code(record.record.state),
        content_ref: record.record.content_ref.clone(),
        evidence_ref: source_trace.and_then(|trace| trace.evidence_ref.clone()),
        source_origin: source_trace.map(|trace| source_origin_code(trace.origin)),
        source_session_id: source_trace.and_then(|trace| trace.session_id.map(|id| id.0)),
        source_cell_id: source_trace.and_then(|trace| trace.cell_id.map(|id| id.0)),
        memory_summary: source_trace.map(|trace| trace.summary.clone()),
        tombstone_blocked_ref: tombstone.map(|value| value.blocked_ref.clone()),
        has_tombstone_reason: tombstone.is_some_and(|value| !value.reason.trim().is_empty()),
        namespace_ref: record.namespace_ref.clone(),
        repo_ref: record.repo_ref.clone(),
        user_ref: record.user_ref.clone(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        last_active_at_ms: record.last_active_at_ms,
        ttl_expires_at_ms: record.ttl_expires_at_ms,
        confidence_basis_points: record.confidence_basis_points,
        importance_basis_points: record.importance_basis_points,
        archived_at_ms: record.archived_at_ms,
        vector_indexed: record.vector_id.is_some(),
        prompt_text_persisted: record.prompt_text_persisted,
        raw_payload_persisted: record.raw_payload_persisted,
        remote_write_allowed: record.remote_write_allowed,
    }
}

pub(super) const fn memory_state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}

const fn source_origin_code(origin: MemorySourceOrigin) -> &'static str {
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
