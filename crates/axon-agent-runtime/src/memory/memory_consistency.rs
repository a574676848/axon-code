//! Memory 物理投影一致性健康诊断。

use std::collections::BTreeSet;

use axon_domain::{AxonError, AxonResult};

use crate::{MemoryOrchestrator, MemoryPhysicalConsistencyRepairInput};

/// Memory 物理一致性诊断报告原因。
pub const MEMORY_CONSISTENCY_REASON_BOUNDED_PROBE: &str = "memory_consistency_bounded_probe";
/// 默认物理投影探测记录数。
pub const MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS: usize = 64;
/// Memory 物理一致性诊断输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPhysicalConsistencyCheckInput<'a> {
    /// Memory 编排器。
    pub orchestrator: &'a MemoryOrchestrator,
    /// 当前时间。
    pub now_ms: u64,
    /// 最大探测记录数；0 使用默认值。
    pub max_probe_records: usize,
}

/// Memory 物理一致性诊断报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryPhysicalConsistencyCheckReport {
    /// SQLite 可召回记录数。
    pub sqlite_recallable_records: usize,
    /// 已探测记录数。
    pub checked_records: usize,
    /// 本轮 SQLite/Tantivy/HNSW 探测预算。
    pub probe_budget_records: usize,
    /// 是否因上限截断探测。
    pub probe_truncated: bool,
    /// Tantivy 未能通过 content_ref 找回的记录数。
    pub text_index_missing_records: usize,
    /// Tantivy 文本索引是否无法读取或查询。
    pub text_index_unreadable: bool,
    /// SQLite 中带 vector_id 但缺失 embedding 投影的记录数。
    pub vector_records_deferred: usize,
    /// HNSW 镜像是否无法读取或解析。
    pub vector_index_unreadable: bool,
    /// HNSW 镜像缺失的 SQLite vector_id 记录数。
    pub vector_index_missing_records: usize,
    /// HNSW 镜像记录数；未启用 embedding 或不可读时为 0。
    pub vector_index_records: usize,
    /// 权威表中已不存在的孤儿向量投影数（有界探测）。
    pub orphan_projection_records: usize,
    /// 权威表中已不存在的孤儿文本索引文档数（有界探测）。
    pub orphan_text_documents: usize,
    /// 是否建议执行物理修复。
    pub repair_recommended: bool,
    /// 是否仍保持 ref-only。
    pub ref_only: bool,
    /// 是否允许远控写入。
    pub remote_write_allowed: bool,
    /// 报告原因。
    pub reason: &'static str,
}

/// Memory 物理一致性诊断 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryPhysicalConsistencyCheckRuntime;

impl MemoryPhysicalConsistencyCheckRuntime {
    /// 对 SQLite、Tantivy 与 HNSW 物理投影做有界健康探测。
    pub fn check(
        &self,
        input: &MemoryPhysicalConsistencyCheckInput<'_>,
    ) -> AxonResult<MemoryPhysicalConsistencyCheckReport> {
        validate_input(input)?;
        let probe_limit = probe_limit(input.max_probe_records);
        let sqlite_store = input.orchestrator.sqlite_store()?;
        let sqlite_recallable_records = sqlite_store.recallable_record_count(input.now_ms)?;
        let recallable = sqlite_store.recallable_records_limited(input.now_ms, probe_limit + 1)?;
        let checked_records = sqlite_recallable_records.min(probe_limit);
        let (text_index_missing_records, text_index_unreadable) =
            text_index_drift(input, &recallable, checked_records);
        let vector_drift = vector_index_drift(input, &recallable, checked_records);
        let orphan_projection_records = orphan_projection_count(input, probe_limit);
        let orphan_text_documents = orphan_text_document_count(input, probe_limit);
        let repair_recommended = text_index_missing_records > 0
            || text_index_unreadable
            || vector_drift.unreadable
            || vector_drift.missing_records > 0
            || vector_drift.projection_missing_records > 0
            || orphan_projection_records > 0
            || orphan_text_documents > 0;
        Ok(MemoryPhysicalConsistencyCheckReport {
            sqlite_recallable_records,
            checked_records,
            probe_budget_records: probe_limit,
            probe_truncated: sqlite_recallable_records > checked_records,
            text_index_missing_records,
            text_index_unreadable,
            vector_records_deferred: vector_drift.projection_missing_records,
            vector_index_unreadable: vector_drift.unreadable,
            vector_index_missing_records: vector_drift.missing_records,
            vector_index_records: vector_drift.records,
            orphan_projection_records,
            orphan_text_documents,
            repair_recommended,
            ref_only: recallable
                .iter()
                .take(checked_records)
                .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted),
            remote_write_allowed: recallable
                .iter()
                .take(checked_records)
                .any(|record| record.remote_write_allowed),
            reason: MEMORY_CONSISTENCY_REASON_BOUNDED_PROBE,
        })
    }
}

impl<'a> From<MemoryPhysicalConsistencyRepairInput<'a>>
    for MemoryPhysicalConsistencyCheckInput<'a>
{
    fn from(input: MemoryPhysicalConsistencyRepairInput<'a>) -> Self {
        Self {
            orchestrator: input.orchestrator,
            now_ms: input.now_ms,
            max_probe_records: MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS,
        }
    }
}

fn validate_input(input: &MemoryPhysicalConsistencyCheckInput<'_>) -> AxonResult<()> {
    if input.now_ms == 0 {
        return Err(AxonError::runtime_failure("Memory 一致性诊断输入无效"));
    }
    Ok(())
}

const fn probe_limit(max_probe_records: usize) -> usize {
    if max_probe_records == 0 {
        MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS
    } else {
        max_probe_records
    }
}

fn text_index_missing_count(
    input: &MemoryPhysicalConsistencyCheckInput<'_>,
    recallable: &[crate::MemorySqliteRecord],
    checked_records: usize,
) -> AxonResult<usize> {
    let index = input.orchestrator.text_index()?;
    let mut missing_ids = BTreeSet::new();
    for record in recallable.iter().take(checked_records) {
        let Some(content_ref) = record.record.content_ref.as_deref() else {
            continue;
        };
        let hits = index.search(
            &text_probe_query(content_ref),
            MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS,
        )?;
        if !hits.iter().any(|hit| hit.memory_id == record.record.id) {
            missing_ids.insert(record.record.id.0);
        }
    }
    Ok(missing_ids.len())
}

fn text_index_drift(
    input: &MemoryPhysicalConsistencyCheckInput<'_>,
    recallable: &[crate::MemorySqliteRecord],
    checked_records: usize,
) -> (usize, bool) {
    if !cfg!(feature = "memory-text-index") {
        return (0, false);
    }
    match text_index_missing_count(input, recallable, checked_records) {
        Ok(missing_records) => (missing_records, false),
        Err(_) => (0, true),
    }
}

/// 反向核对孤儿向量投影：权威表里已经没有这条记录，投影仍在。
fn orphan_projection_count(input: &MemoryPhysicalConsistencyCheckInput<'_>, limit: usize) -> usize {
    input
        .orchestrator
        .vector_projection_store()
        .and_then(|store| store.orphan_projections(limit))
        .map_or(0, |projections| projections.len())
}

/// 反向核对孤儿文本文档：逐条按 memory_id 回查权威表，命中不到即为残留。
fn orphan_text_document_count(
    input: &MemoryPhysicalConsistencyCheckInput<'_>,
    limit: usize,
) -> usize {
    if !cfg!(feature = "memory-text-index") {
        return 0;
    }
    let Ok(index) = input.orchestrator.text_index() else {
        return 0;
    };
    let Ok(indexed) = index.indexed_memory_ids(limit) else {
        return 0;
    };
    let Ok(store) = input.orchestrator.sqlite_store() else {
        return 0;
    };
    indexed
        .into_iter()
        .filter(|memory_id| {
            store
                .get_record(*memory_id)
                .map_or(true, |record| record.is_none())
        })
        .count()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct VectorIndexDrift {
    records: usize,
    unreadable: bool,
    missing_records: usize,
    projection_missing_records: usize,
}

fn vector_index_drift(
    input: &MemoryPhysicalConsistencyCheckInput<'_>,
    recallable: &[crate::MemorySqliteRecord],
    checked_records: usize,
) -> VectorIndexDrift {
    if !cfg!(feature = "memory-vector-index") {
        return VectorIndexDrift::default();
    }
    let vector_ids = recallable
        .iter()
        .take(checked_records)
        .filter_map(|record| record.vector_id)
        .collect::<Vec<_>>();
    let projection_ids = input
        .orchestrator
        .vector_projection_store()
        .and_then(|store| store.projections_by_vector_ids(&vector_ids))
        .map(|projections| {
            projections
                .into_iter()
                .map(|projection| projection.vector_id)
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let projection_missing_records = vector_ids
        .iter()
        .filter(|vector_id| !projection_ids.contains(vector_id))
        .count();
    match input.orchestrator.vector_index() {
        Ok(Some(index)) => inspect_vector_index(&index, &vector_ids, projection_missing_records),
        Ok(None) => VectorIndexDrift::default(),
        Err(_) => unreadable_vector_index_drift(&vector_ids, projection_missing_records),
    }
}

fn inspect_vector_index(
    index: &axon_memory::SharedMemoryVectorIndex,
    vector_ids: &[i64],
    projection_missing_records: usize,
) -> VectorIndexDrift {
    let result = index.len().and_then(|records| {
        let missing_records = vector_ids.iter().try_fold(0usize, |count, vector_id| {
            index
                .contains_vector_id(*vector_id)
                .map(|contains| count + usize::from(!contains))
        })?;
        Ok((records, missing_records))
    });
    match result {
        Ok((records, missing_records)) => VectorIndexDrift {
            records,
            unreadable: false,
            missing_records,
            projection_missing_records,
        },
        Err(_) => unreadable_vector_index_drift(vector_ids, projection_missing_records),
    }
}

fn unreadable_vector_index_drift(
    vector_ids: &[i64],
    projection_missing_records: usize,
) -> VectorIndexDrift {
    VectorIndexDrift {
        records: 0,
        unreadable: true,
        missing_records: vector_ids.len(),
        projection_missing_records,
    }
}

fn text_probe_query(content_ref: &str) -> String {
    content_ref
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || !ch.is_ascii() {
                ch
            } else {
                ' '
            }
        })
        .collect::<String>()
}
