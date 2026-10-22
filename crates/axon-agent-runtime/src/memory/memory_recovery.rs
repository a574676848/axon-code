//! Memory 冲突归档恢复治理。

use std::collections::BTreeSet;

use axon_domain::{AxonError, AxonResult, MemoryId, MemoryState};
use axon_memory::MemoryVectorDocument;

use crate::{
    MemoryOrchestrator, MemorySqliteRecord, MemoryTextDocument,
    memory_consolidation::MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED,
};

/// Memory 物理一致性修复报告原因。
pub const MEMORY_PHYSICAL_REPAIR_REASON_SQLITE_REF_ONLY: &str =
    "memory_physical_repair_sqlite_ref_only";
/// Memory 物理一致性修复默认批次大小。
const MEMORY_PHYSICAL_REPAIR_BATCH_SIZE: usize = 256;
/// Memory 冲突归档批量恢复输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictRecoveryInput<'a> {
    /// Memory 编排器。
    pub orchestrator: &'a MemoryOrchestrator,
    /// 目标 memory id 列表。
    pub memory_ids: &'a [u64],
    /// 当前时间。
    pub now_ms: u64,
    /// 是否已确认批量恢复。
    pub batch_approved: bool,
}

/// Memory 冲突归档恢复项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictRecoveryItem {
    /// 目标 memory id。
    pub memory_id: u64,
    /// 是否已恢复为活跃记录。
    pub recovered: bool,
    /// 恢复前是否已归档。
    pub previous_archived: bool,
    /// 恢复后是否已归档。
    pub current_archived: bool,
}

/// Memory 冲突归档批量恢复报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryConflictRecoveryReport {
    /// 恢复项。
    pub items: Vec<MemoryConflictRecoveryItem>,
    /// 是否仍保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
    /// 报告原因。
    pub reason: &'static str,
}

/// Memory 物理 store 一致性修复输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPhysicalConsistencyRepairInput<'a> {
    /// Memory 编排器。
    pub orchestrator: &'a MemoryOrchestrator,
    /// 当前时间。
    pub now_ms: u64,
    /// 最大修复记录数；0 使用默认值。
    pub max_repair_records: usize,
}

/// Memory 物理 store 一致性修复报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPhysicalConsistencyRepairReport {
    /// SQLite 可召回记录数。
    pub sqlite_recallable_records: usize,
    /// 已重建 Tantivy 文档数。
    pub text_index_rebuilt: usize,
    /// SQLite 中带 vector_id 但缺失 embedding 投影的记录数。
    pub vector_records_deferred: usize,
    /// 已重建 HNSW 向量索引记录数。
    pub vector_index_rebuilt: usize,
    /// 已清理的孤儿向量投影数。
    pub orphan_projections_pruned: usize,
    /// 已清理的孤儿文本文档数。
    pub orphan_text_documents_pruned: usize,
    /// 是否因上限截断修复。
    pub repair_truncated: bool,
    /// 是否仍保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
    /// 报告原因。
    pub reason: &'static str,
}

/// Memory 冲突归档恢复 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryConflictRecoveryRuntime;

/// Memory 物理 store 一致性修复 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryPhysicalConsistencyRepairRuntime;

impl MemoryConflictRecoveryRuntime {
    /// 原子恢复冲突归档记录；所有目标先通过校验后才写回。
    pub fn recover(
        &self,
        input: &MemoryConflictRecoveryInput<'_>,
    ) -> AxonResult<MemoryConflictRecoveryReport> {
        validate_input(input)?;
        let store = input.orchestrator.sqlite_store()?;
        let ids = input
            .memory_ids
            .iter()
            .copied()
            .map(MemoryId)
            .collect::<Vec<_>>();
        let mut records = store.records_by_memory_ids(&ids)?;
        if records.len() != ids.len() {
            return Err(AxonError::runtime_failure("未找到目标 Memory 记录"));
        }
        for memory_id in input.memory_ids {
            let record = records
                .iter()
                .find(|record| record.record.id.0 == *memory_id)
                .ok_or_else(|| AxonError::runtime_failure("未找到目标 Memory 记录"))?;
            ensure_recoverable_conflict(record, input.now_ms)?;
        }

        for record in &mut records {
            record.archived_at_ms = None;
            record.record.tombstone = None;
            record.updated_at_ms = input.now_ms;
            record.last_active_at_ms = input.now_ms;
        }
        store.update_governance_records(&records)?;
        Ok(MemoryConflictRecoveryReport {
            items: input
                .memory_ids
                .iter()
                .map(|memory_id| MemoryConflictRecoveryItem {
                    memory_id: *memory_id,
                    recovered: true,
                    previous_archived: true,
                    current_archived: false,
                })
                .collect(),
            ref_only: records
                .iter()
                .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted),
            remote_write_allowed: records.iter().any(|record| record.remote_write_allowed),
            reason: MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED,
        })
    }
}

impl MemoryPhysicalConsistencyRepairRuntime {
    /// 从 SQLite 治理记录修复可由 ref-only 元数据重建的物理投影。
    pub fn repair(
        &self,
        input: &MemoryPhysicalConsistencyRepairInput<'_>,
    ) -> AxonResult<MemoryPhysicalConsistencyRepairReport> {
        validate_physical_repair_input(input)?;
        let repair_limit = physical_repair_limit(input.max_repair_records);
        let sqlite_store = input.orchestrator.sqlite_store()?;
        let sqlite_recallable_records = sqlite_store.recallable_record_count(input.now_ms)?;
        let recallable = sqlite_store.recallable_records_limited(input.now_ms, repair_limit + 1)?;
        let repair_targets = recallable.iter().take(repair_limit).collect::<Vec<_>>();
        let text_documents = repair_targets
            .iter()
            .copied()
            .map(text_document)
            .collect::<Vec<_>>();
        let text_index_rebuilt = if cfg!(feature = "memory-text-index") {
            input
                .orchestrator
                .text_index()?
                .upsert_documents(text_documents.iter())?;
            text_documents.len()
        } else {
            0
        };

        let vector_repair = repair_vector_index(input.orchestrator, &repair_targets)?;
        let orphan_repair = prune_orphan_projections(input.orchestrator, repair_limit)?;
        let ref_only = repair_targets
            .iter()
            .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted);
        let remote_write_allowed = repair_targets
            .iter()
            .any(|record| record.remote_write_allowed);
        Ok(MemoryPhysicalConsistencyRepairReport {
            sqlite_recallable_records,
            text_index_rebuilt,
            vector_records_deferred: vector_repair.missing_projection_records,
            vector_index_rebuilt: vector_repair.rebuilt_records,
            orphan_projections_pruned: orphan_repair.projections_pruned,
            orphan_text_documents_pruned: orphan_repair.text_documents_pruned,
            repair_truncated: sqlite_recallable_records > repair_targets.len(),
            ref_only,
            remote_write_allowed,
            reason: MEMORY_PHYSICAL_REPAIR_REASON_SQLITE_REF_ONLY,
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct OrphanRepairReport {
    projections_pruned: usize,
    text_documents_pruned: usize,
}

/// 清理权威表中已不存在的投影残留：向量投影、文本文档，并重建物理向量索引。
///
/// 只按权威表重建索引无法发现这类残留，因此这里显式做反查：
/// 投影用一次反连接，文本文档按 memory_id 回查，最后用剩余投影整体替换 HNSW。
fn prune_orphan_projections(
    orchestrator: &MemoryOrchestrator,
    limit: usize,
) -> AxonResult<OrphanRepairReport> {
    let projection_store = orchestrator.vector_projection_store()?;
    let orphans = projection_store.orphan_projections(limit)?;
    let orphan_ids = orphans
        .iter()
        .map(|projection| projection.memory_id)
        .collect::<Vec<_>>();
    let projections_pruned = projection_store.delete_projections_by_memory_ids(&orphan_ids)?;

    let store = orchestrator.sqlite_store()?;
    let mut text_documents_pruned = 0usize;
    if cfg!(feature = "memory-text-index") {
        let mut stale_ids = orphan_ids.clone();
        if let Ok(indexed) = orchestrator.text_index()?.indexed_memory_ids(limit) {
            for memory_id in indexed {
                if store.get_record(memory_id)?.is_none() && !stale_ids.contains(&memory_id) {
                    stale_ids.push(memory_id);
                }
            }
        }
        if !stale_ids.is_empty() {
            text_documents_pruned = stale_ids.len();
            orchestrator.text_index()?.delete_documents(&stale_ids)?;
        }
    }

    rebuild_vector_index_from_projections(orchestrator, limit)?;
    Ok(OrphanRepairReport {
        projections_pruned,
        text_documents_pruned,
    })
}

/// 用权威投影整体替换物理向量索引。
///
/// `replace_vectors` 是全量替换语义，因此这里必须喂入完整投影集，不能只喂本轮
/// 修复窗口内的记录，否则窗口之外的向量会被静默丢掉。
fn rebuild_vector_index_from_projections(
    orchestrator: &MemoryOrchestrator,
    limit: usize,
) -> AxonResult<usize> {
    if !cfg!(feature = "memory-vector-index") {
        return Ok(0);
    }
    let projections = orchestrator
        .vector_projection_store()?
        .all_projections(limit)?;
    let documents = projections
        .iter()
        .map(|projection| MemoryVectorDocument {
            vector_id: projection.vector_id,
            embedding: projection.embedding.clone(),
        })
        .collect::<Vec<_>>();
    let rebuilt = documents.len();
    if let Some(index) = orchestrator.vector_index()? {
        index.replace_vectors(documents)?;
    }
    Ok(rebuilt)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct VectorRepairReport {
    rebuilt_records: usize,
    missing_projection_records: usize,
}

fn repair_vector_index(
    orchestrator: &MemoryOrchestrator,
    records: &[&axon_memory::MemorySqliteRecord],
) -> AxonResult<VectorRepairReport> {
    if !cfg!(feature = "memory-vector-index") {
        return Ok(VectorRepairReport::default());
    }
    let vector_ids = records
        .iter()
        .filter_map(|record| record.vector_id)
        .collect::<Vec<_>>();
    if vector_ids.is_empty() {
        return Ok(VectorRepairReport::default());
    }
    let projections = orchestrator
        .vector_projection_store()?
        .projections_by_vector_ids(&vector_ids)?;
    let projected_ids = projections
        .iter()
        .map(|projection| projection.vector_id)
        .collect::<BTreeSet<_>>();
    let documents = projections
        .into_iter()
        .map(|projection| MemoryVectorDocument {
            vector_id: projection.vector_id,
            embedding: projection.embedding,
        })
        .collect::<Vec<_>>();
    if let Some(index) = orchestrator.vector_index()? {
        index.replace_vectors(documents)?;
    }
    Ok(VectorRepairReport {
        rebuilt_records: projected_ids.len(),
        missing_projection_records: vector_ids
            .iter()
            .filter(|vector_id| !projected_ids.contains(vector_id))
            .count(),
    })
}

fn validate_physical_repair_input(
    input: &MemoryPhysicalConsistencyRepairInput<'_>,
) -> AxonResult<()> {
    if input.now_ms == 0 {
        return Err(AxonError::runtime_failure("Memory 修复输入无效"));
    }
    Ok(())
}

const fn physical_repair_limit(max_repair_records: usize) -> usize {
    if max_repair_records == 0 {
        MEMORY_PHYSICAL_REPAIR_BATCH_SIZE
    } else {
        max_repair_records
    }
}

fn validate_input(input: &MemoryConflictRecoveryInput<'_>) -> AxonResult<()> {
    if input.now_ms == 0 || input.memory_ids.is_empty() || input.memory_ids.contains(&0) {
        return Err(AxonError::runtime_failure("Memory 恢复输入无效"));
    }
    if input.memory_ids.len() > 1 && !input.batch_approved {
        return Err(AxonError::runtime_failure("批量恢复 Memory 需要显式确认"));
    }
    let unique = input.memory_ids.iter().collect::<BTreeSet<_>>();
    if unique.len() != input.memory_ids.len() {
        return Err(AxonError::runtime_failure("Memory 批量恢复不能包含重复 ID"));
    }
    Ok(())
}

fn ensure_recoverable_conflict(record: &MemorySqliteRecord, now_ms: u64) -> AxonResult<()> {
    if record.archived_at_ms.is_none()
        || record
            .record
            .tombstone
            .as_ref()
            .is_none_or(|item| item.reason != MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED)
    {
        return Err(AxonError::runtime_failure(
            "只能恢复被冲突治理归档的 Memory",
        ));
    }
    if !is_recallable_candidate(record) {
        return Err(AxonError::runtime_failure(
            "目标 Memory 缺少可恢复 content ref",
        ));
    }
    if record
        .ttl_expires_at_ms
        .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
    {
        return Err(AxonError::runtime_failure(
            "已过期 Memory 不能恢复为活跃记录",
        ));
    }
    Ok(())
}

fn is_recallable_candidate(record: &MemorySqliteRecord) -> bool {
    matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
        && record
            .record
            .content_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn text_document(record: &crate::MemorySqliteRecord) -> MemoryTextDocument {
    let source = record.record.source_trace.as_ref();
    MemoryTextDocument {
        memory_id: record.record.id,
        content_ref: record.record.content_ref.clone(),
        evidence_ref: source.and_then(|trace| trace.evidence_ref.clone()),
        tombstone_ref: record
            .record
            .tombstone
            .as_ref()
            .map(|tombstone| tombstone.blocked_ref.clone()),
        memory_summary: source.map(|trace| trace.summary.clone()),
    }
}
