use axon_agent_runtime::MemorySqliteRecord;
use axon_domain::{
    CellId, MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin,
    MemorySourceTrace, MemoryState, MemoryTombstone, SessionId,
};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ImportRecordRef {
    pub(super) memory_id: u64,
    pub(super) candidate_id: u64,
    pub(super) memory_kind: Option<String>,
    pub(super) state: Option<String>,
    pub(super) content_ref: Option<String>,
    pub(super) source_origin: Option<String>,
    pub(super) source_session_id: Option<u64>,
    pub(super) source_cell_id: Option<u64>,
    pub(super) memory_summary: Option<String>,
    pub(super) evidence_ref: Option<String>,
    pub(super) tombstone_blocked_ref: Option<String>,
    pub(super) has_tombstone_reason: bool,
    pub(super) created_at_ms: u64,
    pub(super) updated_at_ms: u64,
    pub(super) last_active_at_ms: u64,
    pub(super) ttl_expires_at_ms: Option<u64>,
    pub(super) confidence_basis_points: u16,
    pub(super) importance_basis_points: u16,
    pub(super) archived_at_ms: Option<u64>,
    pub(super) prompt_text_persisted: bool,
    pub(super) raw_payload_persisted: bool,
    pub(super) remote_write_allowed: bool,
    pub(super) namespace_ref: Option<String>,
    pub(super) repo_ref: Option<String>,
    pub(super) user_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportRecordSnapshot {
    memory_id: u64,
    candidate_id: u64,
    memory_kind: Option<String>,
    state: Option<String>,
    content_ref: Option<String>,
    evidence_ref: Option<String>,
    source_origin: Option<String>,
    source_session_id: Option<u64>,
    source_cell_id: Option<u64>,
    memory_summary: Option<String>,
    tombstone_blocked_ref: Option<String>,
    has_tombstone_reason: bool,
    created_at_ms: u64,
    updated_at_ms: u64,
    last_active_at_ms: u64,
    ttl_expires_at_ms: Option<u64>,
    confidence_basis_points: u16,
    importance_basis_points: u16,
    archived_at_ms: Option<u64>,
    prompt_text_persisted: bool,
    raw_payload_persisted: bool,
    remote_write_allowed: bool,
    namespace_ref: Option<String>,
    repo_ref: Option<String>,
    user_ref: Option<String>,
}

pub(super) fn import_record_refs(
    records: &[serde_json::Value],
) -> Result<Vec<ImportRecordRef>, &'static str> {
    records
        .iter()
        .map(|value| {
            serde_json::from_value::<ImportRecordSnapshot>(value.clone())
                .map(import_record_ref)
                .map_err(|_| "memory import 记录投影无效")
        })
        .collect()
}

fn import_record_ref(snapshot: ImportRecordSnapshot) -> ImportRecordRef {
    ImportRecordRef {
        memory_id: snapshot.memory_id,
        candidate_id: snapshot.candidate_id,
        memory_kind: snapshot.memory_kind,
        state: snapshot.state,
        content_ref: snapshot.content_ref,
        source_origin: snapshot.source_origin,
        source_session_id: snapshot.source_session_id,
        source_cell_id: snapshot.source_cell_id,
        memory_summary: snapshot.memory_summary,
        evidence_ref: snapshot.evidence_ref,
        tombstone_blocked_ref: snapshot.tombstone_blocked_ref,
        has_tombstone_reason: snapshot.has_tombstone_reason,
        created_at_ms: snapshot.created_at_ms,
        updated_at_ms: snapshot.updated_at_ms,
        last_active_at_ms: snapshot.last_active_at_ms,
        ttl_expires_at_ms: snapshot.ttl_expires_at_ms,
        confidence_basis_points: snapshot.confidence_basis_points,
        importance_basis_points: snapshot.importance_basis_points,
        archived_at_ms: snapshot.archived_at_ms,
        prompt_text_persisted: snapshot.prompt_text_persisted,
        raw_payload_persisted: snapshot.raw_payload_persisted,
        remote_write_allowed: snapshot.remote_write_allowed,
        namespace_ref: snapshot.namespace_ref,
        repo_ref: snapshot.repo_ref,
        user_ref: snapshot.user_ref,
    }
}

pub(super) fn sqlite_record_from_import(
    record: &ImportRecordRef,
) -> Result<MemorySqliteRecord, &'static str> {
    if record.prompt_text_persisted || record.raw_payload_persisted || record.remote_write_allowed {
        return Err("memory import 只接受 ref-only 且远控关闭的记录");
    }
    let state = record
        .state
        .as_deref()
        .ok_or("memory import 记录缺少 state")
        .and_then(memory_state_from_code)?;
    let source_trace = source_trace_from_import(record)?;
    let tombstone = record.tombstone_blocked_ref.clone().map(|blocked_ref| {
        let reason = if record.has_tombstone_reason {
            "imported-ref-only-tombstone".to_owned()
        } else {
            String::new()
        };
        MemoryTombstone {
            blocked_ref,
            reason,
        }
    });
    let memory_kind = record
        .memory_kind
        .as_deref()
        .ok_or("memory import 记录缺少 memoryKind")
        .and_then(memory_kind_from_code)?;
    Ok(MemorySqliteRecord {
        memory_kind,
        record: MemoryRecord {
            id: MemoryId(record.memory_id),
            candidate_id: MemoryCandidateId(record.candidate_id),
            state,
            content_ref: record.content_ref.clone(),
            source_trace,
            tombstone,
        },
        namespace_ref: record.namespace_ref.clone(),
        repo_ref: record.repo_ref.clone(),
        user_ref: record.user_ref.clone(),
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
        last_active_at_ms: record.last_active_at_ms,
        ttl_expires_at_ms: record.ttl_expires_at_ms,
        confidence_basis_points: record.confidence_basis_points,
        importance_basis_points: record.importance_basis_points,
        vector_id: None,
        archived_at_ms: record.archived_at_ms,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    })
}

fn memory_kind_from_code(value: &str) -> Result<MemoryKind, &'static str> {
    MemoryKind::from_code(value).ok_or("memory import memoryKind 无效")
}

fn source_trace_from_import(
    record: &ImportRecordRef,
) -> Result<Option<MemorySourceTrace>, &'static str> {
    let Some(origin) = record.source_origin.as_deref() else {
        return Ok(None);
    };
    let summary = record
        .memory_summary
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or("memory import 记录缺少 memorySummary")?;
    Ok(Some(MemorySourceTrace {
        origin: source_origin_from_code(origin)?,
        session_id: record.source_session_id.map(SessionId),
        cell_id: record.source_cell_id.map(CellId),
        summary,
        evidence_ref: record.evidence_ref.clone(),
    }))
}

pub(super) const fn memory_state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}

fn memory_state_from_code(value: &str) -> Result<MemoryState, &'static str> {
    match value {
        "weak" => Ok(MemoryState::Weak),
        "strong" => Ok(MemoryState::Strong),
        "quarantined" => Ok(MemoryState::Quarantined),
        "tombstone" => Ok(MemoryState::Tombstone),
        _ => Err("memory import state 无效"),
    }
}

fn source_origin_from_code(value: &str) -> Result<MemorySourceOrigin, &'static str> {
    match value {
        "local_timeline" | "localTimeline" => Ok(MemorySourceOrigin::LocalTimeline),
        "agent_user_context" | "agentUserContext" => Ok(MemorySourceOrigin::AgentUserContext),
        "agent_experience" | "agentExperience" => Ok(MemorySourceOrigin::AgentExperience),
        "agent_knowledge" | "agentKnowledge" => Ok(MemorySourceOrigin::AgentKnowledge),
        "agent_procedure" | "agentProcedure" => Ok(MemorySourceOrigin::AgentProcedure),
        "explicit_user_preference" | "explicitUserPreference" => {
            Ok(MemorySourceOrigin::ExplicitUserPreference)
        }
        "debugging_experience" | "debuggingExperience" => {
            Ok(MemorySourceOrigin::DebuggingExperience)
        }
        "remote_upload" | "remoteUpload" => Ok(MemorySourceOrigin::RemoteUpload),
        "command_completion_feedback" | "commandCompletionFeedback" => {
            Ok(MemorySourceOrigin::CommandCompletionFeedback)
        }
        "skill_reflection" | "skillReflection" => Ok(MemorySourceOrigin::SkillReflection),
        _ => Err("memory import sourceOrigin 无效"),
    }
}
