use axon_domain::{
    MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin, MemorySourceTrace,
    MemoryState,
};
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MEMORY_SUPERSESSION_REASON, MemoryConfig,
    MemoryPipelineJobKind, MemoryPipelineJobStatus, MemoryPipelineStateStore, MemorySqliteRecord,
    MemoryTextDocument, MemoryVectorDocument, MemoryVectorProjection, memory_embedding_hash,
};

use crate::{MemoryConsolidationInput, MemoryConsolidationRuntime, MemoryOrchestrator};

const CONSOLIDATION_TEST_NOW_MS: u64 = 20_000_000_000;
const CONSOLIDATION_TEST_RECENT_MS: u64 = CONSOLIDATION_TEST_NOW_MS - 1_000;

#[test]
fn consolidation_retries_projection_cleanup_without_deleting_supersession_audit() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = MemoryOrchestrator::init(
        MemoryConfig::from_config_dir(temp.path(), true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap(),
    )
    .unwrap();
    let store = orchestrator.sqlite_store().unwrap();
    let predecessor = record(71, "旧事实需要被替代", Some(71), 1_000);
    let successor = record(72, "新事实已经验证", None, 2_000);
    store.upsert_record(&predecessor).unwrap();
    store
        .upsert_record_with_supersessions(&successor, &[MemoryId(71)], 2_000)
        .unwrap();
    let pipeline_store = MemoryPipelineStateStore::open(orchestrator.config()).unwrap();
    pipeline_store
        .upsert_job(
            MemoryPipelineJobKind::ExplicitWrite,
            "memory:72",
            MemoryPipelineJobStatus::NeedsRepair,
            2_000,
        )
        .unwrap();

    let embedding = embedding();
    orchestrator
        .text_index()
        .unwrap()
        .upsert_document(&MemoryTextDocument {
            memory_id: MemoryId(71),
            content_ref: predecessor.record.content_ref.clone(),
            evidence_ref: Some("evidence:test:71".to_owned()),
            tombstone_ref: None,
            memory_summary: Some("旧事实需要被替代".to_owned()),
        })
        .unwrap();
    orchestrator
        .vector_projection_store()
        .unwrap()
        .upsert_projection(&MemoryVectorProjection {
            memory_id: MemoryId(71),
            vector_id: 71,
            model_ref: "embedding:test".to_owned(),
            dimension: MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
            embedding_hash: memory_embedding_hash(&embedding),
            embedding: embedding.clone(),
            updated_at_ms: 2_000,
        })
        .unwrap();
    if let Some(vector_index) = orchestrator.vector_index().unwrap() {
        vector_index
            .upsert_vector(MemoryVectorDocument {
                vector_id: 71,
                embedding,
            })
            .unwrap();
    }

    let report = MemoryConsolidationRuntime
        .consolidate(&MemoryConsolidationInput {
            orchestrator: &orchestrator,
            now_ms: 3_000,
        })
        .unwrap();

    assert_eq!(report.projection_pruned_records, 1);
    assert_eq!(
        pipeline_store
            .get_job(MemoryPipelineJobKind::ExplicitWrite, "memory:72")
            .unwrap()
            .unwrap()
            .status,
        MemoryPipelineJobStatus::NeedsRepair
    );
    let archived = store.get_record(MemoryId(71)).unwrap().unwrap();
    assert_eq!(archived.archived_at_ms, Some(2_000));
    assert_eq!(
        archived.record.tombstone.unwrap().reason,
        MEMORY_SUPERSESSION_REASON
    );
    assert_eq!(
        store
            .supersessions_by_successor(MemoryId(72))
            .unwrap()
            .len(),
        1
    );
    assert!(
        orchestrator
            .vector_projection_store()
            .unwrap()
            .projection_by_vector_id(71)
            .unwrap()
            .is_none()
    );
    assert!(
        orchestrator
            .text_index()
            .unwrap()
            .search("旧事实", 10)
            .unwrap()
            .iter()
            .all(|hit| hit.memory_id != MemoryId(71))
    );
    if let Some(vector_index) = orchestrator.vector_index().unwrap() {
        assert!(!vector_index.contains_vector_id(71).unwrap());
    }
}

#[test]
fn consolidation_decays_tentative_and_counts_duplicates_and_conflicts() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = MemoryOrchestrator::init(
        MemoryConfig::from_config_dir(
            temp.path(),
            false,
            MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
        )
        .unwrap(),
    )
    .unwrap();
    let store = orchestrator.sqlite_store().unwrap();
    let mut tentative = record_with_refs(
        81,
        "长期未复现的暂定认识",
        "memory-ref:tentative",
        "evidence:tentative",
        1,
    );
    tentative.record.state = MemoryState::Weak;
    tentative.confidence_basis_points = 1_000;
    tentative.importance_basis_points = 1_000;
    let duplicate_left = record_with_refs(
        82,
        "重复事实",
        "memory-ref:duplicate",
        "evidence:duplicate:left",
        CONSOLIDATION_TEST_RECENT_MS,
    );
    let duplicate_right = record_with_refs(
        83,
        "重复事实",
        "memory-ref:duplicate",
        "evidence:duplicate:right",
        CONSOLIDATION_TEST_RECENT_MS,
    );
    let conflict_left = record_with_refs(
        84,
        "用户偏好简体中文",
        "memory-ref:conflict:left",
        "evidence:preference:language",
        CONSOLIDATION_TEST_RECENT_MS,
    );
    let conflict_right = record_with_refs(
        85,
        "用户偏好英文",
        "memory-ref:conflict:right",
        "evidence:preference:language",
        CONSOLIDATION_TEST_RECENT_MS,
    );
    for record in [
        tentative,
        duplicate_left,
        duplicate_right,
        conflict_left,
        conflict_right,
    ] {
        store.upsert_record(&record).unwrap();
    }

    let report = MemoryConsolidationRuntime
        .consolidate(&MemoryConsolidationInput {
            orchestrator: &orchestrator,
            now_ms: CONSOLIDATION_TEST_NOW_MS,
        })
        .unwrap();

    assert_eq!(report.low_activity_archived, 1);
    assert_eq!(report.duplicate_tombstones, 1);
    assert_eq!(report.conflict_archived, 1);
    assert!(
        store
            .get_record(MemoryId(81))
            .unwrap()
            .unwrap()
            .archived_at_ms
            .is_some()
    );
}

fn record(
    memory_id: u64,
    summary: &str,
    vector_id: Option<i64>,
    now_ms: u64,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: MemoryKind::Knowledge,
        record: MemoryRecord {
            id: MemoryId(memory_id),
            candidate_id: MemoryCandidateId(memory_id),
            state: MemoryState::Strong,
            content_ref: Some(format!("memory-ref:consolidation:{memory_id}")),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::AgentKnowledge,
                session_id: None,
                cell_id: None,
                summary: summary.to_owned(),
                evidence_ref: Some(format!("evidence:test:{memory_id}")),
            }),
            tombstone: None,
        },
        namespace_ref: Some("namespace:test".to_owned()),
        repo_ref: Some("repo:test".to_owned()),
        user_ref: None,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
        last_active_at_ms: now_ms,
        ttl_expires_at_ms: None,
        confidence_basis_points: 9_000,
        importance_basis_points: 9_000,
        vector_id,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

fn record_with_refs(
    memory_id: u64,
    summary: &str,
    content_ref: &str,
    evidence_ref: &str,
    now_ms: u64,
) -> MemorySqliteRecord {
    let mut record = record(memory_id, summary, None, now_ms);
    record.record.content_ref = Some(content_ref.to_owned());
    record.record.source_trace.as_mut().unwrap().evidence_ref = Some(evidence_ref.to_owned());
    record
}

fn embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION];
    embedding[0] = 1.0;
    embedding
}
