//! Memory 物理索引重建。

use axon_domain::{AxonError, AxonResult};

use crate::{MemoryOrchestrator, MemorySqliteRecord, MemoryTextDocument};

/// Memory 索引重建报告。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryIndexRebuildReport {
    /// 扫描记录数。
    pub scanned_records: usize,
    /// 文本索引重建记录数。
    pub text_index_records: usize,
    /// 向量索引缺失记录数。
    pub vector_missing_records: usize,
}

/// Memory 索引重建 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryIndexRebuildRuntime;

impl MemoryIndexRebuildRuntime {
    /// 从 SQLite 主库重建可重放的本机索引。
    pub fn rebuild(
        &self,
        orchestrator: &MemoryOrchestrator,
    ) -> AxonResult<MemoryIndexRebuildReport> {
        let records = orchestrator.sqlite_store()?.records()?;
        let mut report = MemoryIndexRebuildReport {
            scanned_records: records.len(),
            text_index_records: 0,
            vector_missing_records: 0,
        };
        if cfg!(feature = "memory-text-index") {
            let documents = records
                .iter()
                .map(text_document)
                .collect::<AxonResult<Vec<_>>>()?;
            orchestrator
                .text_index()?
                .upsert_documents(documents.iter())?;
            report.text_index_records = documents.len();
        }
        report.vector_missing_records = records
            .iter()
            .filter(|record| record.vector_id.is_some())
            .count();
        Ok(report)
    }
}

fn text_document(record: &MemorySqliteRecord) -> AxonResult<MemoryTextDocument> {
    if record.record.id.0 == 0 {
        return Err(AxonError::memory_persistence("Memory 索引重建记录 ID 无效"));
    }
    let source = record.record.source_trace.as_ref();
    Ok(MemoryTextDocument {
        memory_id: record.record.id,
        content_ref: record.record.content_ref.clone(),
        evidence_ref: source.and_then(|trace| trace.evidence_ref.clone()),
        tombstone_ref: record
            .record
            .tombstone
            .as_ref()
            .map(|tombstone| tombstone.blocked_ref.clone()),
        memory_summary: source.map(|trace| trace.summary.clone()),
    })
}
