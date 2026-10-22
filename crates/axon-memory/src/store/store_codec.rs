//! Memory SQLite 行编码、类型转换与记录校验。

use axon_domain::{
    AxonError, AxonResult, CellId, MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord,
    MemorySourceOrigin, MemorySourceTrace, MemoryState, MemoryTombstone, SessionId,
};

use super::{MAX_BASIS_POINTS, MemorySqliteRecord, SQLITE_FALSE, SQLITE_TRUE};

pub(crate) fn validate_record(record: &MemorySqliteRecord) -> AxonResult<()> {
    if record.prompt_text_persisted || record.raw_payload_persisted || record.remote_write_allowed {
        return Err(AxonError::memory_persistence(
            "Memory SQLite 不能持久化原文、原始 payload 或开放远控写入",
        ));
    }
    if record.record.id.0 == 0 || record.record.candidate_id.0 == 0 {
        return Err(AxonError::memory_persistence("Memory SQLite ID 无效"));
    }
    if record.updated_at_ms < record.created_at_ms
        || record.last_active_at_ms < record.created_at_ms
    {
        return Err(AxonError::memory_persistence("Memory SQLite 时间戳无效"));
    }
    if record.confidence_basis_points > MAX_BASIS_POINTS
        || record.importance_basis_points > MAX_BASIS_POINTS
    {
        return Err(AxonError::memory_persistence(
            "Memory SQLite 置信度或重要度无效",
        ));
    }
    let personal_kind = matches!(
        record.memory_kind,
        MemoryKind::Profile | MemoryKind::Preference
    );
    if personal_kind != record.user_ref.is_some() {
        return Err(AxonError::memory_persistence("Memory kind 与 scope 不匹配"));
    }
    if matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
        && record
            .record
            .content_ref
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(AxonError::memory_persistence(
            "Memory SQLite 可召回记录必须有 content ref",
        ));
    }
    Ok(())
}

pub(crate) fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemorySqliteRecord> {
    let source_origin = row
        .get::<_, Option<String>>("source_origin")?
        .map(|value| origin_from_code(&value))
        .transpose()?;
    let source_session_id = row
        .get::<_, Option<i64>>("source_session_id")?
        .map(u64_from_i64)
        .transpose()?
        .map(SessionId);
    let source_cell_id = row
        .get::<_, Option<i64>>("source_cell_id")?
        .map(u64_from_i64)
        .transpose()?
        .map(CellId);
    let memory_summary = row.get::<_, Option<String>>("memory_summary")?;
    let evidence_ref = row.get::<_, Option<String>>("evidence_ref")?;
    let tombstone_blocked_ref = row.get::<_, Option<String>>("tombstone_blocked_ref")?;
    let tombstone_reason = row.get::<_, Option<String>>("tombstone_reason")?;
    Ok(MemorySqliteRecord {
        record: MemoryRecord {
            id: MemoryId(u64_from_i64(row.get("memory_id")?)?),
            candidate_id: MemoryCandidateId(u64_from_i64(row.get("candidate_id")?)?),
            state: state_from_code(&row.get::<_, String>("state")?)?,
            content_ref: row.get("content_ref")?,
            source_trace: source_origin.map(|origin| MemorySourceTrace {
                origin,
                session_id: source_session_id,
                cell_id: source_cell_id,
                summary: memory_summary.unwrap_or_default(),
                evidence_ref,
            }),
            tombstone: tombstone_blocked_ref.map(|blocked_ref| MemoryTombstone {
                blocked_ref,
                reason: tombstone_reason.unwrap_or_default(),
            }),
        },
        memory_kind: kind_from_code(&row.get::<_, String>("memory_kind")?)?,
        namespace_ref: row.get("namespace_ref")?,
        repo_ref: row.get("repo_ref")?,
        user_ref: row.get("user_ref")?,
        vector_id: row.get("vector_id")?,
        created_at_ms: u64_from_i64(row.get("created_at_ms")?)?,
        updated_at_ms: u64_from_i64(row.get("updated_at_ms")?)?,
        last_active_at_ms: u64_from_i64(row.get("last_active_at_ms")?)?,
        ttl_expires_at_ms: row
            .get::<_, Option<i64>>("ttl_expires_at_ms")?
            .map(u64_from_i64)
            .transpose()?,
        confidence_basis_points: u16_from_i64(row.get("confidence_basis_points")?)?,
        importance_basis_points: u16_from_i64(row.get("importance_basis_points")?)?,
        archived_at_ms: row
            .get::<_, Option<i64>>("archived_at_ms")?
            .map(u64_from_i64)
            .transpose()?,
        prompt_text_persisted: bool_from_i64(row.get("prompt_text_persisted")?)?,
        raw_payload_persisted: bool_from_i64(row.get("raw_payload_persisted")?)?,
        remote_write_allowed: bool_from_i64(row.get("remote_write_allowed")?)?,
    })
}

pub(crate) const fn state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}

fn state_from_code(value: &str) -> rusqlite::Result<MemoryState> {
    match value {
        "weak" => Ok(MemoryState::Weak),
        "strong" => Ok(MemoryState::Strong),
        "quarantined" => Ok(MemoryState::Quarantined),
        "tombstone" => Ok(MemoryState::Tombstone),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn kind_from_code(value: &str) -> rusqlite::Result<MemoryKind> {
    MemoryKind::from_code(value).ok_or(rusqlite::Error::InvalidQuery)
}

pub(crate) const fn origin_code(origin: MemorySourceOrigin) -> &'static str {
    match origin {
        MemorySourceOrigin::LocalTimeline => "localTimeline",
        MemorySourceOrigin::AgentUserContext => "agentUserContext",
        MemorySourceOrigin::AgentExperience => "agentExperience",
        MemorySourceOrigin::AgentKnowledge => "agentKnowledge",
        MemorySourceOrigin::AgentProcedure => "agentProcedure",
        MemorySourceOrigin::ExplicitUserPreference => "explicitUserPreference",
        MemorySourceOrigin::DebuggingExperience => "debuggingExperience",
        MemorySourceOrigin::RemoteUpload => "remoteUpload",
        MemorySourceOrigin::CommandCompletionFeedback => "commandCompletionFeedback",
        MemorySourceOrigin::SkillReflection => "skillReflection",
    }
}

fn origin_from_code(value: &str) -> rusqlite::Result<MemorySourceOrigin> {
    match value {
        "localTimeline" => Ok(MemorySourceOrigin::LocalTimeline),
        "agentUserContext" => Ok(MemorySourceOrigin::AgentUserContext),
        "agentExperience" => Ok(MemorySourceOrigin::AgentExperience),
        "agentKnowledge" => Ok(MemorySourceOrigin::AgentKnowledge),
        "agentProcedure" => Ok(MemorySourceOrigin::AgentProcedure),
        "explicitUserPreference" => Ok(MemorySourceOrigin::ExplicitUserPreference),
        "debuggingExperience" => Ok(MemorySourceOrigin::DebuggingExperience),
        "remoteUpload" => Ok(MemorySourceOrigin::RemoteUpload),
        "commandCompletionFeedback" => Ok(MemorySourceOrigin::CommandCompletionFeedback),
        "skillReflection" => Ok(MemorySourceOrigin::SkillReflection),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(crate) const fn sqlite_bool(value: bool) -> i64 {
    if value { SQLITE_TRUE } else { SQLITE_FALSE }
}

pub(crate) fn bool_from_i64(value: i64) -> rusqlite::Result<bool> {
    match value {
        SQLITE_FALSE => Ok(false),
        SQLITE_TRUE => Ok(true),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(crate) fn optional_i64_from_u64(value: Option<u64>) -> AxonResult<Option<i64>> {
    value.map(i64_from_u64).transpose()
}

pub(crate) fn i64_from_u64(value: u64) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))
}

pub(crate) fn i64_from_usize(value: usize) -> AxonResult<i64> {
    i64::try_from(value).map_err(|_| AxonError::memory_persistence("Memory SQLite 数值溢出"))
}

pub(crate) fn u64_from_i64(value: i64) -> rusqlite::Result<u64> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn usize_from_i64(value: i64) -> rusqlite::Result<usize> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}

pub(crate) fn unique_positive_i64s(values: impl Iterator<Item = u64>) -> AxonResult<Vec<i64>> {
    let mut values = values
        .map(i64_from_u64)
        .collect::<AxonResult<Vec<_>>>()?
        .into_iter()
        .filter(|value| *value > 0)
        .collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

pub(crate) fn unique_positive_i64s_from_i64(values: impl Iterator<Item = i64>) -> Vec<i64> {
    let mut values = values.filter(|value| *value > 0).collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    values
}

fn u16_from_i64(value: i64) -> rusqlite::Result<u16> {
    value.try_into().map_err(|_| rusqlite::Error::InvalidQuery)
}
