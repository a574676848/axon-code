//! Memory 睡眠期巩固与冲突治理。

use std::collections::{BTreeMap, BTreeSet};

use axon_domain::{AxonError, AxonResult, MemoryId, MemoryState};

use crate::{
    MemoryOrchestrator, MemorySqliteRecord,
    memory_consolidation_policy::{
        archive_conflicting_records, archive_low_activity_records, conflict_key, conflict_record,
        has_conflicting_content, is_active_recallable, is_expired_record,
        tombstone_duplicate_records, validate_consolidation_input,
    },
};

const MEMORY_ARCHIVE_STATUS_ACTIVE: &str = "active";
const MEMORY_ARCHIVE_STATUS_ARCHIVED: &str = "archived";

/// 重复记忆合并的 tombstone 原因。
pub const MEMORY_CONSOLIDATION_REASON_DUPLICATE_MERGED: &str = "重复记忆已被更高置信记录合并";
/// 冲突旧事实归档原因。
pub const MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED: &str = "同源事实已被更新证据覆盖";
/// 巩固报告安全原因。
pub const MEMORY_CONSOLIDATION_REASON_REF_ONLY: &str =
    "memory consolidation persisted ref-only governance";
/// TTL 清理解释码。
pub const MEMORY_CONSOLIDATION_EXPLANATION_TTL_CLEANUP: &str = "ttl_cleanup";
/// 低活性归档解释码。
pub const MEMORY_CONSOLIDATION_EXPLANATION_LOW_ACTIVITY_ARCHIVE: &str = "low_activity_archive";
/// 重复合并解释码。
pub const MEMORY_CONSOLIDATION_EXPLANATION_DUPLICATE_MERGE: &str = "duplicate_merge";
/// 冲突旧事实归档解释码。
pub const MEMORY_CONSOLIDATION_EXPLANATION_CONFLICT_ARCHIVE: &str = "conflict_archive";
/// 不可召回投影清理解释码。
pub const MEMORY_CONSOLIDATION_EXPLANATION_PROJECTION_CLEANUP: &str = "projection_cleanup";
/// 默认巩固间隔，单位毫秒。
pub const MEMORY_CONSOLIDATION_DEFAULT_INTERVAL_MS: u64 = 3_600_000;
/// Memory 巩固输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConsolidationInput<'a> {
    /// Memory 编排器。
    pub orchestrator: &'a MemoryOrchestrator,
    /// 当前时间。
    pub now_ms: u64,
}

/// Memory 巩固报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConsolidationReport {
    /// TTL 物理清理数量。
    pub ttl_cleaned: usize,
    /// 低活性归档数量。
    pub low_activity_archived: usize,
    /// 重复记忆 tombstone 数量。
    pub duplicate_tombstones: usize,
    /// 冲突旧事实归档数量。
    pub conflict_archived: usize,
    /// 已兜底清理投影的不可召回记录数量。
    pub projection_pruned_records: usize,
    /// 巩固后活跃记录数量。
    pub active_records: usize,
    /// 是否仍保持 ref-only 安全边界。
    pub ref_only: bool,
    /// 是否阻断远控写入。
    pub remote_write_allowed: bool,
    /// 报告原因。
    pub reason: &'static str,
    /// 面向 reviewer 的 ref-only 巩固解释。
    pub explanations: Vec<MemoryConsolidationExplanation>,
}

/// Memory 巩固解释条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConsolidationExplanation {
    /// 治理动作码。
    pub action: &'static str,
    /// 受影响记录数。
    pub affected_records: usize,
    /// 人类可读解释。
    pub summary: &'static str,
}

/// Memory 冲突查询输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictListInput<'a> {
    /// Memory 编排器。
    pub orchestrator: &'a MemoryOrchestrator,
    /// 返回冲突组上限。
    pub limit: usize,
}

/// Memory 冲突组。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictGroup {
    /// 来源范围与证据 key。
    pub conflict_key: String,
    /// 组内记录。
    pub records: Vec<MemoryConflictRecord>,
}

/// Memory 冲突记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictRecord {
    /// 记忆 ID。
    pub memory_id: u64,
    /// 内容引用。
    pub content_ref: Option<String>,
    /// namespace scope 引用。
    pub namespace_ref: Option<String>,
    /// repo scope 引用。
    pub repo_ref: Option<String>,
    /// user scope 引用。
    pub user_ref: Option<String>,
    /// 是否已归档。
    pub archived: bool,
    /// 治理状态。
    pub state: MemoryState,
    /// 是否为冲突归档记录。
    pub recoverable_conflict: bool,
    /// 更新时间。
    pub updated_at_ms: u64,
    /// 置信度。
    pub confidence_basis_points: u16,
    /// 重要度。
    pub importance_basis_points: u16,
}

impl MemoryConflictRecord {
    /// 返回稳定生命周期状态码。
    #[must_use]
    pub const fn lifecycle_status(&self) -> &'static str {
        if self.archived {
            MEMORY_ARCHIVE_STATUS_ARCHIVED
        } else {
            MEMORY_ARCHIVE_STATUS_ACTIVE
        }
    }
}

/// Memory 冲突查询报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictListReport {
    /// 冲突组数量。
    pub conflict_group_count: usize,
    /// 冲突组。
    pub groups: Vec<MemoryConflictGroup>,
    /// 是否仍保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
}

/// Memory 睡眠期巩固 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryConsolidationRuntime;

impl MemoryConsolidationRuntime {
    /// 执行 TTL 清理、低活性归档、重复合并与冲突旧事实归档。
    pub fn consolidate(
        &self,
        input: &MemoryConsolidationInput<'_>,
    ) -> AxonResult<MemoryConsolidationReport> {
        validate_consolidation_input(input)?;

        let sqlite_store = input.orchestrator.sqlite_store()?;
        let before_records = sqlite_store.records()?;
        let before_ids = before_records
            .iter()
            .map(|record| record.record.id)
            .collect::<Vec<_>>();
        let before_ttl = before_records.len();
        let records = before_records
            .clone()
            .into_iter()
            .filter(|record| !is_expired_record(record, input.now_ms))
            .collect::<Vec<_>>();
        let ttl_cleaned = before_ttl.saturating_sub(records.len());
        let (records, low_activity_archived) = archive_low_activity_records(records, input.now_ms);
        let (records, duplicate_tombstones) = tombstone_duplicate_records(records, input.now_ms);
        let (records, conflict_archived) = archive_conflicting_records(records, input.now_ms);

        persist_consolidated_records(&sqlite_store, &records, &before_ids)?;
        let projection_pruned_records =
            prune_non_recallable_projections(input.orchestrator, &before_records, &records)?;

        let active_records = records
            .iter()
            .filter(|record| record.archived_at_ms.is_none())
            .count();
        let ref_only = records
            .iter()
            .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted);
        let remote_write_allowed = records.iter().any(|record| record.remote_write_allowed);
        Ok(MemoryConsolidationReport {
            ttl_cleaned,
            low_activity_archived,
            duplicate_tombstones,
            conflict_archived,
            projection_pruned_records,
            active_records,
            ref_only,
            remote_write_allowed,
            reason: MEMORY_CONSOLIDATION_REASON_REF_ONLY,
            explanations: consolidation_explanations(
                ttl_cleaned,
                low_activity_archived,
                duplicate_tombstones,
                conflict_archived,
                projection_pruned_records,
            ),
        })
    }

    /// 只读列出同来源范围内的冲突事实组。
    pub fn list_conflicts(
        &self,
        input: &MemoryConflictListInput<'_>,
    ) -> AxonResult<MemoryConflictListReport> {
        if input.limit == 0 {
            return Err(AxonError::runtime_failure("Memory 冲突查询输入无效"));
        }
        let records = input.orchestrator.sqlite_store()?.records()?;
        let mut groups = BTreeMap::<String, Vec<&MemorySqliteRecord>>::new();
        for record in records.iter().filter(|record| {
            is_active_recallable(record)
                || record.record.tombstone.as_ref().is_some_and(|item| {
                    item.reason == MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED
                })
        }) {
            let Some(key) = conflict_key(record) else {
                continue;
            };
            groups.entry(key).or_default().push(record);
        }
        let mut groups = groups
            .into_iter()
            .filter(|(_, records)| has_conflicting_content(records))
            .map(|(conflict_key, records)| MemoryConflictGroup {
                conflict_key,
                records: records.into_iter().map(conflict_record).collect(),
            })
            .collect::<Vec<_>>();
        groups.sort_by_key(|group| {
            std::cmp::Reverse(
                group
                    .records
                    .iter()
                    .map(|record| record.updated_at_ms)
                    .max()
                    .unwrap_or_default(),
            )
        });
        groups.truncate(input.limit);
        Ok(MemoryConflictListReport {
            conflict_group_count: groups.len(),
            groups,
            ref_only: records
                .iter()
                .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted),
            remote_write_allowed: records.iter().any(|record| record.remote_write_allowed),
        })
    }
}

fn consolidation_explanations(
    ttl_cleaned: usize,
    low_activity_archived: usize,
    duplicate_tombstones: usize,
    conflict_archived: usize,
    projection_pruned_records: usize,
) -> Vec<MemoryConsolidationExplanation> {
    let mut explanations = Vec::new();
    push_explanation(
        &mut explanations,
        ttl_cleaned,
        MEMORY_CONSOLIDATION_EXPLANATION_TTL_CLEANUP,
        "清理已过期记忆，避免超过 TTL 的事实继续参与召回",
    );
    push_explanation(
        &mut explanations,
        low_activity_archived,
        MEMORY_CONSOLIDATION_EXPLANATION_LOW_ACTIVITY_ARCHIVE,
        "归档低活性记忆，降低长期未使用事实的召回干扰",
    );
    push_explanation(
        &mut explanations,
        duplicate_tombstones,
        MEMORY_CONSOLIDATION_EXPLANATION_DUPLICATE_MERGE,
        "合并重复内容引用，保留置信度和活跃度更高的记忆",
    );
    push_explanation(
        &mut explanations,
        conflict_archived,
        MEMORY_CONSOLIDATION_EXPLANATION_CONFLICT_ARCHIVE,
        "归档同证据链旧事实，保留更新或更高置信记录",
    );
    push_explanation(
        &mut explanations,
        projection_pruned_records,
        MEMORY_CONSOLIDATION_EXPLANATION_PROJECTION_CLEANUP,
        "清理不可召回记录的文本与向量投影，兜底修复写入后的投影漂移",
    );
    explanations
}

fn push_explanation(
    explanations: &mut Vec<MemoryConsolidationExplanation>,
    affected_records: usize,
    action: &'static str,
    summary: &'static str,
) {
    if affected_records > 0 {
        explanations.push(MemoryConsolidationExplanation {
            action,
            affected_records,
            summary,
        });
    }
}

fn prune_non_recallable_projections(
    orchestrator: &MemoryOrchestrator,
    before_records: &[MemorySqliteRecord],
    retained_records: &[MemorySqliteRecord],
) -> AxonResult<usize> {
    let active_ids = retained_records
        .iter()
        .filter(|record| is_active_recallable(record))
        .map(|record| record.record.id)
        .collect::<BTreeSet<_>>();
    let records = before_records
        .iter()
        .filter(|record| !active_ids.contains(&record.record.id))
        .collect::<Vec<_>>();
    if records.is_empty() {
        return Ok(0);
    }
    let memory_ids = records
        .iter()
        .map(|record| record.record.id)
        .collect::<Vec<MemoryId>>();
    let vector_ids = records
        .iter()
        .filter_map(|record| record.vector_id)
        .collect::<Vec<_>>();
    orchestrator.text_index()?.delete_documents(&memory_ids)?;
    orchestrator
        .vector_projection_store()?
        .delete_projections_by_memory_ids(&memory_ids)?;
    if let Some(vector_index) = orchestrator.vector_index()? {
        vector_index.remove_vectors(&vector_ids)?;
    }
    Ok(memory_ids.len())
}

fn persist_consolidated_records(
    store: &crate::MemorySqliteStore,
    records: &[MemorySqliteRecord],
    before_ids: &[axon_domain::MemoryId],
) -> AxonResult<()> {
    let retained_ids = records
        .iter()
        .map(|record| record.record.id)
        .collect::<std::collections::BTreeSet<_>>();
    let expired_ids = before_ids
        .iter()
        .copied()
        .filter(|memory_id| !retained_ids.contains(memory_id))
        .collect::<Vec<_>>();
    store.update_governance_records(records)?;
    store.delete_records(&expired_ids)?;
    Ok(())
}
