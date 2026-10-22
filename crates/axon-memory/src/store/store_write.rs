//! Memory SQLite 记录写入参数封装。

use axon_domain::AxonResult;
use rusqlite::{Connection, params};

use crate::store::{
    MemorySqliteRecord, i64_from_u64, optional_i64_from_u64, origin_code, sqlite_bool, state_code,
    validate_record,
};

const UPSERT_MEMORY_RECORD_SQL: &str = r"
    INSERT INTO memory_records (
        memory_id, candidate_id, memory_kind, state, content_ref, source_origin,
        source_session_id, source_cell_id, memory_summary, evidence_ref,
        tombstone_blocked_ref, tombstone_reason, namespace_ref, repo_ref, user_ref,
        vector_id, created_at_ms, updated_at_ms, last_active_at_ms, ttl_expires_at_ms,
        confidence_basis_points, importance_basis_points, archived_at_ms,
        prompt_text_persisted, raw_payload_persisted, remote_write_allowed
    ) VALUES (
        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
        ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
    )
    ON CONFLICT(memory_id) DO UPDATE SET
        candidate_id = excluded.candidate_id,
        memory_kind = excluded.memory_kind,
        state = excluded.state,
        content_ref = excluded.content_ref,
        source_origin = excluded.source_origin,
        source_session_id = excluded.source_session_id,
        source_cell_id = excluded.source_cell_id,
        memory_summary = excluded.memory_summary,
        evidence_ref = excluded.evidence_ref,
        tombstone_blocked_ref = excluded.tombstone_blocked_ref,
        tombstone_reason = excluded.tombstone_reason,
        namespace_ref = excluded.namespace_ref,
        repo_ref = excluded.repo_ref,
        user_ref = excluded.user_ref,
        vector_id = excluded.vector_id,
        updated_at_ms = excluded.updated_at_ms,
        last_active_at_ms = excluded.last_active_at_ms,
        ttl_expires_at_ms = excluded.ttl_expires_at_ms,
        confidence_basis_points = excluded.confidence_basis_points,
        importance_basis_points = excluded.importance_basis_points,
        archived_at_ms = excluded.archived_at_ms,
        prompt_text_persisted = excluded.prompt_text_persisted,
        raw_payload_persisted = excluded.raw_payload_persisted,
        remote_write_allowed = excluded.remote_write_allowed
    ";

pub(crate) fn upsert_record_on_connection(
    connection: &Connection,
    record: &MemorySqliteRecord,
) -> AxonResult<()> {
    validate_record(record)?;
    let source = record.record.source_trace.as_ref();
    let tombstone = record.record.tombstone.as_ref();
    connection
        .execute(
            UPSERT_MEMORY_RECORD_SQL,
            params![
                i64_from_u64(record.record.id.0)?,
                i64_from_u64(record.record.candidate_id.0)?,
                record.memory_kind.as_str(),
                state_code(record.record.state),
                record.record.content_ref.as_deref(),
                source.map(|trace| origin_code(trace.origin)),
                optional_i64_from_u64(source.and_then(|trace| trace.session_id.map(|id| id.0)))?,
                optional_i64_from_u64(source.and_then(|trace| trace.cell_id.map(|id| id.0)))?,
                source.map(|trace| trace.summary.as_str()),
                source.and_then(|trace| trace.evidence_ref.as_deref()),
                tombstone.map(|item| item.blocked_ref.as_str()),
                tombstone.map(|item| item.reason.as_str()),
                record.namespace_ref.as_deref(),
                record.repo_ref.as_deref(),
                record.user_ref.as_deref(),
                record.vector_id,
                i64_from_u64(record.created_at_ms)?,
                i64_from_u64(record.updated_at_ms)?,
                i64_from_u64(record.last_active_at_ms)?,
                optional_i64_from_u64(record.ttl_expires_at_ms)?,
                i64::from(record.confidence_basis_points),
                i64::from(record.importance_basis_points),
                optional_i64_from_u64(record.archived_at_ms)?,
                sqlite_bool(record.prompt_text_persisted),
                sqlite_bool(record.raw_payload_persisted),
                sqlite_bool(record.remote_write_allowed),
            ],
        )
        .map_err(|_| axon_domain::AxonError::memory_persistence("Memory SQLite 记录写入失败"))?;
    Ok(())
}
