//! Memory SQLite 测试夹具；被多个测试模块共用，避免夹具分叉。

use super::{CellId, MemoryCandidateId, MemorySqliteRecord, SessionId};
use axon_domain::{
    MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin, MemorySourceTrace, MemoryState,
};

pub(crate) fn scoped_record(
    id: u64,
    user_ref: Option<&str>,
    namespace_ref: &str,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: if user_ref.is_some() {
            MemoryKind::Preference
        } else {
            MemoryKind::Knowledge
        },
        record: MemoryRecord {
            id: MemoryId(id),
            candidate_id: MemoryCandidateId(id),
            state: MemoryState::Strong,
            content_ref: Some(format!("memory-ref:{id}")),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::AgentKnowledge,
                session_id: Some(SessionId(1)),
                cell_id: Some(CellId(id)),
                summary: format!("scope fixture {id}"),
                evidence_ref: Some(format!("evidence:{id}")),
            }),
            tombstone: None,
        },
        namespace_ref: Some(namespace_ref.to_owned()),
        repo_ref: None,
        user_ref: user_ref.map(str::to_owned),
        created_at_ms: 1,
        updated_at_ms: 1,
        last_active_at_ms: 1,
        ttl_expires_at_ms: None,
        confidence_basis_points: 9_000,
        importance_basis_points: 8_000,
        vector_id: None,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

pub(crate) fn personal_context_record(
    id: u64,
    user_ref: &str,
    namespace_ref: &str,
    origin: MemorySourceOrigin,
    summary: &str,
) -> MemorySqliteRecord {
    let mut record = scoped_record(id, Some(user_ref), namespace_ref);
    let source = record
        .record
        .source_trace
        .as_mut()
        .expect("测试记录应包含来源");
    source.origin = origin;
    source.summary = summary.to_owned();
    record
}

pub(crate) fn memory_summaries(records: &[MemorySqliteRecord]) -> Vec<String> {
    records
        .iter()
        .filter_map(|record| record.record.source_trace.as_ref())
        .map(|source| source.summary.clone())
        .collect()
}
