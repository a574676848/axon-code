//! Axon Memory 写路径。
//!
//! 本 crate 承载显式写入入口、写入安全默认值和 Phase1/Phase2 物理工件协调。
//! 运行时只需要把已治理事件转接到这里。

use axon_domain::{
    AxonError, AxonResult, MemoryAdmissionDecision, MemoryAdmissionGate, MemoryCandidate, MemoryId,
    MemoryKind, MemoryRecord, MemoryWriteDecision,
};
use axon_memory::{
    MemoryFileLock, MemoryOrchestrator, MemoryPipelineJobKind, MemoryPipelineJobStatus,
    MemoryPipelineStateStore, MemorySqliteRecord, MemoryTextDocument, MemoryVectorDocument,
    MemoryVectorProjection, memory_embedding_hash,
};

mod scope;

pub use scope::{
    MemoryWriteScope, memory_scope_refs, memory_scope_refs_with_namespace, personal_memory_scope,
};

/// Memory 写入默认重要度，单位 basis point。
pub const MEMORY_WRITE_TRIGGER_DEFAULT_IMPORTANCE_BASIS_POINTS: u16 = 8_000;
/// Memory 写入最大 basis point。
const MEMORY_WRITE_TRIGGER_MAX_BASIS_POINTS: u16 = 10_000;
/// Memory 写入流水线 sidecar 锁文件名。
const MEMORY_WRITE_TRIGGER_PIPELINE_LOCK_FILE: &str = "write_pipeline";
/// Memory 写入后同步刷入 HNSW 的向量投影批量窗口。
const MEMORY_WRITE_TRIGGER_VECTOR_FLUSH_LIMIT: usize = 64;

/// Memory footer 可复用准入观测。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWriteAdmissionObservation {
    /// 候选 ID。
    pub candidate_id: u64,
    /// 准入原因。
    pub reason: String,
    /// 是否允许写入。
    pub accepted: bool,
}

/// Agent Loop Memory 写入输入。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryWriteTriggerInput {
    /// 候选记忆；必须已经由上游闭环构造器生成，不接受 prompt 原文。
    pub candidate: MemoryCandidate,
    /// memory_write 的稳定五值类型。
    pub memory_kind: MemoryKind,
    /// 目标 Memory ID。
    pub memory_id: MemoryId,
    /// 被当前结论明确替代的旧 Memory ID。
    pub supersedes_memory_ids: Vec<MemoryId>,
    /// 记忆作用域；所有生产写入必须显式提供。
    pub scope: MemoryWriteScope,
    /// 当前时间。
    pub now_ms: u64,
    /// TTL 过期时间。
    pub ttl_expires_at_ms: Option<u64>,
    /// 重要度。
    pub importance_basis_points: u16,
    /// 可选 embedding；只写本机 HNSW 镜像，不进入远控。
    pub embedding: Option<Vec<f32>>,
    /// embedding 模型引用；提供 embedding 时必须同步提供。
    pub embedding_model_ref: Option<String>,
}

/// Agent Loop Memory 写入报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWriteTriggerReport {
    /// 准入决策。
    pub admission: MemoryAdmissionDecision,
    /// footer/inspector 可复用的准入观测。
    pub observation: MemoryWriteAdmissionObservation,
    /// 已写入的治理记录；拒绝或待复核时为空。
    pub record: Option<MemoryRecord>,
    /// 是否合并/强化已有重复记录，而非新增。
    pub merged_existing: bool,
    /// 命中的重复记录 ID。
    pub duplicate_memory_id: Option<MemoryId>,
    /// 已建立语义替代关系并归档的旧 Memory ID。
    pub superseded_memory_ids: Vec<MemoryId>,
    /// 语义替代后的文本投影是否已清理；没有替代关系时为空。
    pub supersession_text_index_pruned: Option<bool>,
    /// 语义替代后的向量投影是否已清理；没有替代关系时为空。
    pub supersession_vector_projection_pruned: Option<bool>,
    /// 语义替代后的 HNSW 投影是否已清理；没有替代关系时为空。
    pub supersession_vector_index_pruned: Option<bool>,
    /// 是否写入 SQLite。
    pub sqlite_written: bool,
    /// 是否写入 Tantivy。
    pub text_index_written: bool,
    /// 是否写入 HNSW。
    pub vector_index_written: bool,
    /// 是否写入 embedding 投影。
    pub vector_projection_written: bool,
    /// 是否写入 pipeline 状态。
    pub pipeline_state_written: bool,
    /// 本次写入的 pipeline 终态。
    pub pipeline_status: MemoryPipelineJobStatus,
    /// SQLite 权威写入成功后，是否仍有派生投影需要异步修复。
    pub needs_repair: bool,
}

/// Agent Loop Memory 写入触发 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryWriteTriggerRuntime;

impl MemoryWriteTriggerRuntime {
    /// 将闭环候选写入本机 Memory 物理 store。
    pub fn trigger(
        &self,
        orchestrator: &MemoryOrchestrator,
        input: &MemoryWriteTriggerInput,
    ) -> AxonResult<MemoryWriteTriggerReport> {
        validate_input(input, orchestrator.config().dimension)?;
        if matches!(
            input.candidate.sensitivity,
            axon_domain::MemorySensitivity::Personal
        ) && input.scope.user_ref.is_none()
        {
            return Err(AxonError::invalid_input(
                "Personal Memory 必须绑定 owner principal，Coding Agent 不能直接写入",
            ));
        }
        let _pipeline_lock = MemoryFileLock::acquire(
            &orchestrator
                .config()
                .memory_dir
                .join(MEMORY_WRITE_TRIGGER_PIPELINE_LOCK_FILE),
        )?;
        let admission = MemoryAdmissionGate::evaluate_candidate(
            &input.candidate,
            axon_domain::MemoryAdmissionPolicy::default(),
        );
        let observation = admission_observation(&input.candidate, admission);
        let _ = mark_write_job(orchestrator, input, MemoryPipelineJobStatus::Running);
        let Some(record) =
            MemoryAdmissionGate::record_from_decision(input.memory_id, &input.candidate, admission)
        else {
            let state_written =
                mark_write_job(orchestrator, input, MemoryPipelineJobStatus::Succeeded);
            return Ok(rejected_report(admission, observation, state_written));
        };

        let ttl_expires_at_ms = input.ttl_expires_at_ms;
        let write_result = match duplicate_record(orchestrator, &input.candidate) {
            Ok(Some(duplicate)) => merge_duplicate_record(
                orchestrator,
                input,
                admission,
                observation,
                duplicate,
                ttl_expires_at_ms,
            ),
            Ok(None) => write_new_record(
                orchestrator,
                input,
                admission,
                observation,
                record,
                ttl_expires_at_ms,
            ),
            Err(error) => Err(error),
        };
        if write_result.is_err() {
            let _ = mark_write_job(orchestrator, input, MemoryPipelineJobStatus::Failed);
        }
        write_result
    }
}

fn rejected_report(
    admission: MemoryAdmissionDecision,
    observation: MemoryWriteAdmissionObservation,
    pipeline_state_written: bool,
) -> MemoryWriteTriggerReport {
    MemoryWriteTriggerReport {
        admission,
        observation,
        record: None,
        merged_existing: false,
        duplicate_memory_id: None,
        superseded_memory_ids: Vec::new(),
        supersession_text_index_pruned: None,
        supersession_vector_projection_pruned: None,
        supersession_vector_index_pruned: None,
        sqlite_written: false,
        text_index_written: false,
        vector_index_written: false,
        vector_projection_written: false,
        pipeline_state_written,
        pipeline_status: MemoryPipelineJobStatus::Succeeded,
        needs_repair: false,
    }
}

fn merge_duplicate_record(
    orchestrator: &MemoryOrchestrator,
    input: &MemoryWriteTriggerInput,
    admission: MemoryAdmissionDecision,
    observation: MemoryWriteAdmissionObservation,
    mut merged: MemorySqliteRecord,
    ttl_expires_at_ms: Option<u64>,
) -> AxonResult<MemoryWriteTriggerReport> {
    ensure_record_scope(&merged, &input.scope)?;
    merged.updated_at_ms = input.now_ms;
    merged.last_active_at_ms = input.now_ms;
    merged.confidence_basis_points = merged
        .confidence_basis_points
        .max(input.candidate.confidence_basis_points);
    merged.importance_basis_points = merged
        .importance_basis_points
        .max(input.importance_basis_points);
    merged.ttl_expires_at_ms = merged.ttl_expires_at_ms.or(ttl_expires_at_ms);
    let supersession = persist_record(orchestrator, input, &merged)?;
    let pipeline_status = supersession.pipeline_status();
    let pipeline_state_written = mark_write_job(orchestrator, input, pipeline_status);
    Ok(MemoryWriteTriggerReport {
        admission,
        observation,
        record: Some(merged.record.clone()),
        merged_existing: true,
        duplicate_memory_id: Some(merged.record.id),
        superseded_memory_ids: supersession.memory_ids,
        supersession_text_index_pruned: supersession.text_index_pruned,
        supersession_vector_projection_pruned: supersession.vector_projection_pruned,
        supersession_vector_index_pruned: supersession.vector_index_pruned,
        sqlite_written: true,
        text_index_written: false,
        vector_index_written: false,
        vector_projection_written: false,
        pipeline_state_written,
        pipeline_status,
        needs_repair: pipeline_status == MemoryPipelineJobStatus::NeedsRepair,
    })
}

fn write_new_record(
    orchestrator: &MemoryOrchestrator,
    input: &MemoryWriteTriggerInput,
    admission: MemoryAdmissionDecision,
    observation: MemoryWriteAdmissionObservation,
    record: MemoryRecord,
    ttl_expires_at_ms: Option<u64>,
) -> AxonResult<MemoryWriteTriggerReport> {
    let vector_id = input
        .embedding
        .as_ref()
        .map(|_| vector_id_from_memory_id(input.memory_id))
        .transpose()?;
    let sqlite_record = sqlite_record(input, &record, ttl_expires_at_ms, vector_id);
    let supersession = persist_record(orchestrator, input, &sqlite_record)?;
    let text_index_written = write_text_index(orchestrator, &record);
    let vector_projection_written = write_vector_projection(orchestrator, input, vector_id);
    let vector_index_written = write_vector_index(orchestrator, input.embedding.clone(), vector_id);
    let pipeline_status = write_pipeline_status(
        cfg!(feature = "memory-text-index"),
        text_index_written,
        vector_write_required(input),
        vector_projection_written,
        vector_index_written,
        supersession.pipeline_ready(),
    );
    let pipeline_state_written = mark_write_job(orchestrator, input, pipeline_status);
    Ok(MemoryWriteTriggerReport {
        admission,
        observation,
        record: Some(record),
        merged_existing: false,
        duplicate_memory_id: None,
        superseded_memory_ids: supersession.memory_ids,
        supersession_text_index_pruned: supersession.text_index_pruned,
        supersession_vector_projection_pruned: supersession.vector_projection_pruned,
        supersession_vector_index_pruned: supersession.vector_index_pruned,
        sqlite_written: true,
        text_index_written,
        vector_index_written,
        vector_projection_written,
        pipeline_state_written,
        pipeline_status,
        needs_repair: pipeline_status == MemoryPipelineJobStatus::NeedsRepair,
    })
}

fn sqlite_record(
    input: &MemoryWriteTriggerInput,
    record: &MemoryRecord,
    ttl_expires_at_ms: Option<u64>,
    vector_id: Option<i64>,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        record: record.clone(),
        memory_kind: input.memory_kind,
        namespace_ref: Some(input.scope.namespace_ref.clone()),
        repo_ref: input.scope.repo_ref.clone(),
        user_ref: input.scope.user_ref.clone(),
        created_at_ms: input.now_ms,
        updated_at_ms: input.now_ms,
        last_active_at_ms: input.now_ms,
        ttl_expires_at_ms,
        confidence_basis_points: input.candidate.confidence_basis_points,
        importance_basis_points: input.importance_basis_points,
        vector_id,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

fn write_text_index(orchestrator: &MemoryOrchestrator, record: &MemoryRecord) -> bool {
    cfg!(feature = "memory-text-index")
        && orchestrator
            .text_index()
            .and_then(|index| index.upsert_document(&text_document(record)))
            .is_ok()
}

const fn write_pipeline_status(
    text_write_required: bool,
    text_index_written: bool,
    vector_write_required: bool,
    vector_projection_written: bool,
    vector_index_written: bool,
    supersession_projection_ready: bool,
) -> MemoryPipelineJobStatus {
    if (text_write_required && !text_index_written)
        || (vector_write_required && (!vector_projection_written || !vector_index_written))
        || !supersession_projection_ready
    {
        MemoryPipelineJobStatus::NeedsRepair
    } else {
        MemoryPipelineJobStatus::Succeeded
    }
}

fn vector_write_required(input: &MemoryWriteTriggerInput) -> bool {
    input.embedding.is_some()
}

fn write_vector_index(
    orchestrator: &MemoryOrchestrator,
    embedding: Option<Vec<f32>>,
    vector_id: Option<i64>,
) -> bool {
    if !cfg!(feature = "memory-vector-index") {
        return false;
    }
    let (Some(embedding), Some(vector_id)) = (embedding, vector_id) else {
        return false;
    };
    let documents = vector_flush_documents(orchestrator, vector_id, embedding);
    match orchestrator.vector_index() {
        Ok(Some(vector_index)) => vector_index.upsert_vectors(documents).is_ok(),
        _ => false,
    }
}

fn vector_flush_documents(
    orchestrator: &MemoryOrchestrator,
    vector_id: i64,
    embedding: Vec<f32>,
) -> Vec<MemoryVectorDocument> {
    let mut documents = orchestrator
        .vector_projection_store()
        .and_then(|store| store.recent_projections(MEMORY_WRITE_TRIGGER_VECTOR_FLUSH_LIMIT))
        .ok()
        .map(|projections| {
            projections
                .into_iter()
                .map(|projection| MemoryVectorDocument {
                    vector_id: projection.vector_id,
                    embedding: projection.embedding,
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !documents
        .iter()
        .any(|document| document.vector_id == vector_id)
    {
        documents.push(MemoryVectorDocument {
            vector_id,
            embedding,
        });
    }
    documents
}

fn write_vector_projection(
    orchestrator: &MemoryOrchestrator,
    input: &MemoryWriteTriggerInput,
    vector_id: Option<i64>,
) -> bool {
    let (Some(embedding), Some(vector_id), Some(model_ref)) = (
        input.embedding.as_ref(),
        vector_id,
        input.embedding_model_ref.as_deref(),
    ) else {
        return input.embedding.is_none();
    };
    orchestrator
        .vector_projection_store()
        .and_then(|store| {
            store.upsert_projection(&MemoryVectorProjection {
                memory_id: input.memory_id,
                vector_id,
                model_ref: model_ref.to_owned(),
                dimension: orchestrator.config().dimension,
                embedding_hash: memory_embedding_hash(embedding),
                embedding: embedding.clone(),
                updated_at_ms: input.now_ms,
            })
        })
        .is_ok()
}

fn mark_write_job(
    orchestrator: &MemoryOrchestrator,
    input: &MemoryWriteTriggerInput,
    status: MemoryPipelineJobStatus,
) -> bool {
    MemoryPipelineStateStore::open(orchestrator.config())
        .and_then(|store| {
            store.upsert_job(
                MemoryPipelineJobKind::ExplicitWrite,
                &format!("memory:{}", input.memory_id.0),
                status,
                input.now_ms,
            )
        })
        .is_ok()
}

fn duplicate_record(
    orchestrator: &MemoryOrchestrator,
    candidate: &MemoryCandidate,
) -> AxonResult<Option<MemorySqliteRecord>> {
    orchestrator
        .sqlite_store()?
        .find_active_by_content_ref(&candidate.content_ref)
}

#[derive(Debug, Default)]
struct SupersessionProjectionReport {
    memory_ids: Vec<MemoryId>,
    text_index_pruned: Option<bool>,
    vector_projection_pruned: Option<bool>,
    vector_index_pruned: Option<bool>,
}

impl SupersessionProjectionReport {
    fn pipeline_ready(&self) -> bool {
        self.text_index_pruned.unwrap_or(true)
            && self.vector_projection_pruned.unwrap_or(true)
            && self.vector_index_pruned.unwrap_or(true)
    }

    fn pipeline_status(&self) -> MemoryPipelineJobStatus {
        if self.pipeline_ready() {
            MemoryPipelineJobStatus::Succeeded
        } else {
            MemoryPipelineJobStatus::NeedsRepair
        }
    }
}

fn persist_record(
    orchestrator: &MemoryOrchestrator,
    input: &MemoryWriteTriggerInput,
    record: &MemorySqliteRecord,
) -> AxonResult<SupersessionProjectionReport> {
    let store = orchestrator.sqlite_store()?;
    if input.supersedes_memory_ids.is_empty() {
        store.upsert_record(record)?;
        return Ok(SupersessionProjectionReport::default());
    }
    let relations = store.upsert_record_with_supersessions(
        record,
        &input.supersedes_memory_ids,
        input.now_ms,
    )?;
    let memory_ids = relations
        .iter()
        .map(|relation| relation.predecessor_memory_id)
        .collect::<Vec<_>>();
    let predecessors = store.records_by_memory_ids(&memory_ids)?;
    let vector_ids = predecessors
        .iter()
        .filter_map(|predecessor| predecessor.vector_id)
        .collect::<Vec<_>>();
    let text_index_pruned = !cfg!(feature = "memory-text-index")
        || orchestrator
            .text_index()
            .and_then(|index| index.delete_documents(&memory_ids))
            .is_ok();
    let vector_projection_pruned = orchestrator
        .vector_projection_store()
        .and_then(|store| store.delete_projections_by_memory_ids(&memory_ids))
        .is_ok();
    let vector_index_pruned = !cfg!(feature = "memory-vector-index")
        || vector_ids.is_empty()
        || orchestrator
            .vector_index()
            .and_then(|index| {
                index.ok_or_else(|| AxonError::memory_persistence("Memory HNSW 索引不可用"))
            })
            .and_then(|index| index.remove_vectors(&vector_ids))
            .is_ok();
    Ok(SupersessionProjectionReport {
        memory_ids,
        text_index_pruned: Some(text_index_pruned),
        vector_projection_pruned: Some(vector_projection_pruned),
        vector_index_pruned: Some(vector_index_pruned),
    })
}

fn ensure_record_scope(record: &MemorySqliteRecord, scope: &MemoryWriteScope) -> AxonResult<()> {
    if record.namespace_ref.as_deref() != Some(scope.namespace_ref.as_str())
        || record.repo_ref != scope.repo_ref
        || record.user_ref != scope.user_ref
    {
        return Err(AxonError::invalid_input("Memory 写入不能跨越作用域合并"));
    }
    Ok(())
}

fn validate_input(input: &MemoryWriteTriggerInput, dimension: usize) -> AxonResult<()> {
    if input.memory_id.0 == 0 || input.now_ms == 0 {
        return Err(AxonError::invalid_input(
            "Memory 写入触发输入 ID 或时间无效",
        ));
    }
    if let Some(ttl) = input.ttl_expires_at_ms
        && ttl <= input.now_ms
    {
        return Err(AxonError::invalid_input("Memory 写入 TTL 必须晚于当前时间"));
    }
    if input.importance_basis_points > MEMORY_WRITE_TRIGGER_MAX_BASIS_POINTS {
        return Err(AxonError::invalid_input("Memory 写入重要度无效"));
    }
    if input.scope.namespace_ref.trim().is_empty() {
        return Err(AxonError::invalid_input(
            "Memory 写入 namespace scope 不能为空",
        ));
    }
    if let Some(embedding) = input.embedding.as_deref() {
        validate_embedding(embedding, dimension)?;
        if input
            .embedding_model_ref
            .as_ref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(AxonError::invalid_input(
                "Memory 写入 embedding 必须携带模型引用",
            ));
        }
    } else if input.embedding_model_ref.is_some() {
        return Err(AxonError::invalid_input(
            "Memory 写入没有 embedding 时不能携带模型引用",
        ));
    }
    Ok(())
}

fn validate_embedding(embedding: &[f32], dimension: usize) -> AxonResult<()> {
    if embedding.len() != dimension {
        return Err(AxonError::invalid_input(
            "Memory 写入 embedding 维度与配置不一致",
        ));
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(AxonError::invalid_input(
            "Memory 写入 embedding 包含非法数值",
        ));
    }
    if embedding.iter().all(|value| *value == 0.0) {
        return Err(AxonError::invalid_input(
            "Memory 写入 embedding 不能是全零向量",
        ));
    }
    Ok(())
}

fn vector_id_from_memory_id(memory_id: MemoryId) -> AxonResult<i64> {
    i64::try_from(memory_id.0).map_err(|_| AxonError::invalid_input("Memory 写入 vector_id 溢出"))
}

fn text_document(record: &MemoryRecord) -> MemoryTextDocument {
    let source = record.source_trace.as_ref();
    MemoryTextDocument {
        memory_id: record.id,
        content_ref: record.content_ref.clone(),
        evidence_ref: source.and_then(|trace| trace.evidence_ref.clone()),
        tombstone_ref: record
            .tombstone
            .as_ref()
            .map(|tombstone| tombstone.blocked_ref.clone()),
        memory_summary: source.map(|trace| trace.summary.clone()),
    }
}

fn admission_observation(
    candidate: &MemoryCandidate,
    admission: MemoryAdmissionDecision,
) -> MemoryWriteAdmissionObservation {
    MemoryWriteAdmissionObservation {
        candidate_id: candidate.id.0,
        reason: format!("{:?}", admission.reason),
        accepted: matches!(
            admission.decision,
            MemoryWriteDecision::WriteStrong
                | MemoryWriteDecision::WriteWeak
                | MemoryWriteDecision::ForgetTombstone
                | MemoryWriteDecision::Quarantine
        ),
    }
}

#[cfg(test)]
#[path = "memory_write_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "memory_write_limit_tests.rs"]
mod memory_write_limit_tests;
