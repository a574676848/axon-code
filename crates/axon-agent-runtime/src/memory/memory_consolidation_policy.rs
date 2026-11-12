//! Memory 巩固纯治理策略。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult, MemoryState, MemoryTombstone};

pub(super) use axon_memory::is_active_recallable;
pub use axon_memory::{MEMORY_DEFAULT_HALF_LIFE_DAYS, decayed_score_basis_points};

use crate::{
    MemorySqliteRecord,
    memory_consolidation::{
        MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED, MemoryConflictRecord,
        MemoryConsolidationInput,
    },
};

const MEMORY_DEFAULT_NAMESPACE_REF: &str = "memory-scope:local";

pub(super) fn validate_consolidation_input(input: &MemoryConsolidationInput<'_>) -> AxonResult<()> {
    if input.now_ms == 0 {
        return Err(AxonError::runtime_failure("Memory 巩固输入无效"));
    }
    Ok(())
}

pub(super) fn is_expired_record(record: &MemorySqliteRecord, now_ms: u64) -> bool {
    record
        .ttl_expires_at_ms
        .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
}

/// 低活性归档：判定口径统一取 `axon_memory::aging::should_archive`。
///
/// 该判定自带活跃性前置条件（未归档 + Weak/Strong + content_ref 非空），
/// 因此墓碑记录与空引用记录不会被重复改写 `updated_at_ms`。
pub(super) fn archive_low_activity_records(
    records: Vec<MemorySqliteRecord>,
    now_ms: u64,
) -> (Vec<MemorySqliteRecord>, usize) {
    let mut archived = 0usize;
    let records = records
        .into_iter()
        .map(|mut record| {
            if axon_memory::should_archive(&record, now_ms) {
                record.archived_at_ms = Some(now_ms);
                record.updated_at_ms = now_ms;
                archived = archived.saturating_add(1);
            }
            record
        })
        .collect();
    (records, archived)
}

/// 归档同证据链里的旧事实，只保留更新或更高置信的那条。
///
/// 这是 `memory recover-conflict` 的唯一数据来源，不能因为命中窗口窄就下线。
pub(super) fn archive_conflicting_records(
    mut records: Vec<MemorySqliteRecord>,
    now_ms: u64,
) -> (Vec<MemorySqliteRecord>, usize) {
    let mut conflict_count = 0usize;
    let mut best_by_evidence = BTreeMap::<String, usize>::new();

    for index in 0..records.len() {
        if !is_active_recallable(&records[index]) {
            continue;
        }
        let Some(key) = conflict_key(&records[index]) else {
            continue;
        };
        if let Some(existing_index) = best_by_evidence.get(&key).copied() {
            if records[existing_index].record.content_ref == records[index].record.content_ref {
                continue;
            }
            let winner = better_record_index(&records, existing_index, index, now_ms);
            let loser = if winner == existing_index {
                index
            } else {
                best_by_evidence.insert(key, index);
                existing_index
            };
            archive_record(
                &mut records[loser],
                MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED,
                now_ms,
            );
            conflict_count = conflict_count.saturating_add(1);
        } else {
            best_by_evidence.insert(key, index);
        }
    }

    (records, conflict_count)
}

pub(super) fn conflict_key(record: &MemorySqliteRecord) -> Option<String> {
    let source = record.record.source_trace.as_ref()?;
    let evidence_key = source
        .evidence_ref
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .or_else(|| (!source.summary.trim().is_empty()).then(|| source.summary.clone()))?;
    Some(format!(
        "{}:{}:{}:{:?}:{}:{}:{evidence_key}",
        record
            .namespace_ref
            .as_deref()
            .unwrap_or(MEMORY_DEFAULT_NAMESPACE_REF),
        record.repo_ref.as_deref().unwrap_or("repo-scope:any"),
        record.user_ref.as_deref().unwrap_or("user-scope:any"),
        source.origin,
        source.session_id.map_or(0, |id| id.0),
        source.cell_id.map_or(0, |id| id.0)
    ))
}

pub(super) fn has_conflicting_content(records: &[&MemorySqliteRecord]) -> bool {
    let mut content_refs = records
        .iter()
        .filter_map(|record| record.record.content_ref.as_deref())
        .collect::<Vec<_>>();
    content_refs.sort_unstable();
    content_refs.dedup();
    content_refs.len() > 1
}

pub(super) fn conflict_record(record: &MemorySqliteRecord) -> MemoryConflictRecord {
    MemoryConflictRecord {
        memory_id: record.record.id.0,
        content_ref: record.record.content_ref.clone(),
        namespace_ref: record.namespace_ref.clone(),
        repo_ref: record.repo_ref.clone(),
        user_ref: record.user_ref.clone(),
        archived: record.archived_at_ms.is_some(),
        state: record.record.state,
        recoverable_conflict: record.record.tombstone.as_ref().is_some_and(|item| {
            item.reason == MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED
                && record.record.content_ref.is_some()
        }),
        updated_at_ms: record.updated_at_ms,
        confidence_basis_points: record.confidence_basis_points,
        importance_basis_points: record.importance_basis_points,
    }
}

fn better_record_index(
    records: &[MemorySqliteRecord],
    left: usize,
    right: usize,
    now_ms: u64,
) -> usize {
    let left_record = &records[left];
    let right_record = &records[right];
    let left_rank = record_rank(left_record, now_ms);
    let right_rank = record_rank(right_record, now_ms);
    if right_rank > left_rank { right } else { left }
}

fn record_rank(record: &MemorySqliteRecord, now_ms: u64) -> (u8, u16, u64, u64) {
    (
        state_rank(record.record.state),
        decayed_score_basis_points(record, now_ms),
        record.updated_at_ms,
        record.record.id.0,
    )
}

const fn state_rank(state: MemoryState) -> u8 {
    match state {
        MemoryState::Strong => 2,
        MemoryState::Weak => 1,
        MemoryState::Quarantined | MemoryState::Tombstone => 0,
    }
}

fn archive_record(record: &mut MemorySqliteRecord, reason: &'static str, now_ms: u64) {
    let blocked_ref = record.record.content_ref.clone().unwrap_or_default();
    record.record.tombstone = Some(MemoryTombstone {
        blocked_ref,
        reason: reason.to_owned(),
    });
    record.archived_at_ms = Some(now_ms);
    record.updated_at_ms = now_ms;
}
