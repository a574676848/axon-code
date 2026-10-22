//! Memory 巩固纯治理策略。

use std::collections::BTreeMap;

use axon_domain::{AxonError, AxonResult, MemoryState, MemoryTombstone};

use crate::{
    MemorySqliteRecord,
    memory_consolidation::{
        MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED,
        MEMORY_CONSOLIDATION_REASON_DUPLICATE_MERGED, MemoryConflictRecord,
        MemoryConsolidationInput,
    },
    memory_governance_policy::MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS,
};

const MEMORY_DEFAULT_NAMESPACE_REF: &str = "memory-scope:local";
const MAX_BASIS_POINTS: u16 = 10_000;
/// 记忆衰减半衰期，单位天；治理、合并与界面展示共用同一口径。
pub const MEMORY_DEFAULT_HALF_LIFE_DAYS: u64 = 30;
const SECONDS_PER_DAY: u64 = 86_400;

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

pub(super) fn archive_low_activity_records(
    records: Vec<MemorySqliteRecord>,
    now_ms: u64,
) -> (Vec<MemorySqliteRecord>, usize) {
    let mut archived = 0usize;
    let records = records
        .into_iter()
        .map(|mut record| {
            let score = decayed_score_basis_points(&record, now_ms);
            if record.archived_at_ms.is_none()
                && score < MEMORY_ARCHIVE_SCORE_THRESHOLD_BASIS_POINTS
            {
                record.archived_at_ms = Some(now_ms);
                record.updated_at_ms = now_ms;
                archived = archived.saturating_add(1);
            }
            record
        })
        .collect();
    (records, archived)
}

pub(super) fn tombstone_duplicate_records(
    mut records: Vec<MemorySqliteRecord>,
    now_ms: u64,
) -> (Vec<MemorySqliteRecord>, usize) {
    let mut duplicate_count = 0usize;
    let mut best_by_content_ref = BTreeMap::<String, usize>::new();

    for index in 0..records.len() {
        if !is_active_recallable(&records[index]) {
            continue;
        }
        let Some(content_ref) = records[index].record.content_ref.clone() else {
            continue;
        };
        if let Some(existing_index) = best_by_content_ref.get(&content_ref).copied() {
            let winner = better_record_index(&records, existing_index, index, now_ms);
            let loser = if winner == existing_index {
                index
            } else {
                best_by_content_ref.insert(content_ref.clone(), index);
                existing_index
            };
            mark_tombstone(
                &mut records[loser],
                content_ref,
                MEMORY_CONSOLIDATION_REASON_DUPLICATE_MERGED,
                now_ms,
            );
            duplicate_count = duplicate_count.saturating_add(1);
        } else {
            best_by_content_ref.insert(content_ref, index);
        }
    }

    (records, duplicate_count)
}

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

pub(super) fn is_active_recallable(record: &MemorySqliteRecord) -> bool {
    record.archived_at_ms.is_none()
        && matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
        && record
            .record
            .content_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
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

fn mark_tombstone(
    record: &mut MemorySqliteRecord,
    blocked_ref: String,
    reason: &'static str,
    now_ms: u64,
) {
    record.record.state = MemoryState::Tombstone;
    record.record.content_ref = None;
    record.record.tombstone = Some(MemoryTombstone {
        blocked_ref,
        reason: reason.to_owned(),
    });
    record.archived_at_ms = Some(now_ms);
    record.updated_at_ms = now_ms;
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

/// 计算衰减后的有效权重（basis point）：置信度与重要度之积按半衰期指数衰减。
pub fn decayed_score_basis_points(record: &MemorySqliteRecord, now_ms: u64) -> u16 {
    let elapsed_ms =
        std::time::Duration::from_millis(now_ms.saturating_sub(record.last_active_at_ms))
            .as_secs_f64()
            * 1_000.0;
    let half_life_ms = std::time::Duration::from_secs(
        MEMORY_DEFAULT_HALF_LIFE_DAYS.saturating_mul(SECONDS_PER_DAY),
    )
    .as_secs_f64()
        * 1_000.0;
    let base = f64::from(record.confidence_basis_points)
        * f64::from(record.importance_basis_points)
        / f64::from(MAX_BASIS_POINTS);
    let decay = (-std::f64::consts::LN_2 * elapsed_ms / half_life_ms).exp();
    rounded_basis_points(base * decay)
}

fn rounded_basis_points(score: f64) -> u16 {
    let target = score.round().clamp(0.0, f64::from(MAX_BASIS_POINTS));
    let mut lower = 0_u16;
    let mut upper = MAX_BASIS_POINTS;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        if f64::from(middle) < target {
            lower = middle + 1;
        } else {
            upper = middle;
        }
    }
    lower
}
