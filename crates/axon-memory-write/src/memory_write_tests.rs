use axon_domain::{
    CellId, MemoryCandidateId, MemoryCandidateKind, MemoryRetentionPolicy, MemorySensitivity,
    MemorySourceOrigin, MemorySourceTrace, MemoryVerification, SessionId,
};
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MEMORY_SUPERSESSION_REASON, MemoryConfig,
    MemoryHybridQuery, MemoryPipelineJobKind, MemoryPipelineJobStatus,
};

use super::*;

#[test]
fn write_pipeline_status_tracks_required_projection_failures() {
    assert_eq!(
        write_pipeline_status(true, false, false, true, true, true),
        MemoryPipelineJobStatus::NeedsRepair
    );
    assert_eq!(
        write_pipeline_status(false, false, true, false, true, true),
        MemoryPipelineJobStatus::NeedsRepair
    );
    assert_eq!(
        write_pipeline_status(false, false, true, true, false, true),
        MemoryPipelineJobStatus::NeedsRepair
    );
    assert_eq!(
        write_pipeline_status(false, false, false, true, true, false),
        MemoryPipelineJobStatus::NeedsRepair
    );
    assert_eq!(
        write_pipeline_status(true, true, true, true, true, true),
        MemoryPipelineJobStatus::Succeeded
    );
}

#[test]
fn write_trigger_persists_embedding_projection_before_vector_index() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = orchestrator(&temp);
    let embedding = valid_embedding();
    let report = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                candidate(1, "写入测试闭环验证通过"),
                10,
                scope("namespace:test"),
                Vec::new(),
                Some(embedding.clone()),
                Some("embedding:model".to_owned()),
                1_000,
            ),
        )
        .unwrap();

    assert!(report.sqlite_written);
    assert!(report.vector_projection_written);
    let projection = orchestrator
        .vector_projection_store()
        .unwrap()
        .projection_by_vector_id(10)
        .unwrap()
        .expect("embedding 投影应存在");
    assert_eq!(projection.embedding_hash, memory_embedding_hash(&embedding));
    let job = MemoryPipelineStateStore::open(orchestrator.config())
        .unwrap()
        .get_job(MemoryPipelineJobKind::ExplicitWrite, "memory:10")
        .unwrap()
        .expect("写入 job 应存在");
    if cfg!(feature = "memory-vector-index") {
        assert_eq!(job.status, MemoryPipelineJobStatus::Succeeded);
        assert_eq!(report.pipeline_status, MemoryPipelineJobStatus::Succeeded);
        assert!(!report.needs_repair);
        let reopened = orchestrator
            .vector_index()
            .unwrap()
            .expect("embedding 已启用");
        assert!(reopened.contains_vector_id(10).unwrap());
    } else {
        assert_eq!(job.status, MemoryPipelineJobStatus::NeedsRepair);
        assert_eq!(report.pipeline_status, MemoryPipelineJobStatus::NeedsRepair);
        assert!(report.needs_repair);
    }
}

#[test]
fn write_trigger_rejects_embedding_without_model_ref() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = orchestrator(&temp);
    let error = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                candidate(2, "缺少模型引用"),
                20,
                scope("namespace:test"),
                Vec::new(),
                Some(valid_embedding()),
                None,
                1_000,
            ),
        )
        .expect_err("缺少模型引用必须拒绝");

    assert!(format!("{error:?}").contains("embedding 必须携带模型引用"));
}

#[cfg(feature = "memory-vector-index")]
#[test]
fn write_trigger_returns_needs_repair_when_vector_index_is_unavailable() {
    let temp = axon_test_support::test_temp_dir();
    let config =
        MemoryConfig::from_config_dir(temp.path(), true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap();
    std::fs::create_dir_all(&config.hnsw_index_path).unwrap();
    let orchestrator = MemoryOrchestrator::init(config.clone()).unwrap();
    let report = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                candidate(21, "向量投影故障注入"),
                210,
                scope("namespace:projection-failure"),
                Vec::new(),
                Some(valid_embedding()),
                Some("embedding:model".to_owned()),
                1_000,
            ),
        )
        .unwrap();

    assert!(report.sqlite_written);
    assert!(report.vector_projection_written);
    assert!(report.needs_repair);
    assert_eq!(
        orchestrator
            .sqlite_store()
            .unwrap()
            .get_record(MemoryId(210))
            .unwrap()
            .unwrap()
            .memory_kind,
        axon_domain::MemoryKind::Procedure
    );
}

#[test]
fn personal_candidate_rejects_workspace_scope() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = orchestrator(&temp);
    let mut personal = candidate(91, "个人偏好");
    personal.sensitivity = MemorySensitivity::Personal;
    let error = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                personal,
                91,
                scope("workspace:test"),
                Vec::new(),
                None,
                None,
                1_000,
            ),
        )
        .expect_err("Personal candidate 不得写入 Workspace scope");
    assert!(format!("{error:?}").contains("Personal Memory 必须绑定 owner principal"));
}

#[test]
fn semantic_supersession_archives_old_memory_and_is_idempotent() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = orchestrator(&temp);
    let shared_scope = scope("namespace:semantic");
    MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                candidate(3, "旧事实：服务只支持本地运行"),
                30,
                shared_scope.clone(),
                Vec::new(),
                None,
                None,
                1_000,
            ),
        )
        .unwrap();
    let replacement = input(
        candidate(4, "新事实：服务同时支持本地和远程运行"),
        40,
        shared_scope,
        vec![MemoryId(30)],
        None,
        None,
        2_000,
    );

    let first = MemoryWriteTriggerRuntime
        .trigger(&orchestrator, &replacement)
        .unwrap();
    let retry = MemoryWriteTriggerRuntime
        .trigger(&orchestrator, &replacement)
        .unwrap();

    assert_eq!(first.superseded_memory_ids, vec![MemoryId(30)]);
    assert_eq!(retry.superseded_memory_ids, vec![MemoryId(30)]);
    assert!(!first.needs_repair);
    assert!(!retry.needs_repair);
    assert_eq!(first.pipeline_status, MemoryPipelineJobStatus::Succeeded);
    assert_eq!(retry.pipeline_status, MemoryPipelineJobStatus::Succeeded);
    assert!(retry.merged_existing);
    let store = orchestrator.sqlite_store().unwrap();
    let old = store.get_record(MemoryId(30)).unwrap().unwrap();
    assert_eq!(old.archived_at_ms, Some(2_000));
    assert_eq!(
        old.record.tombstone.unwrap().reason,
        MEMORY_SUPERSESSION_REASON
    );
    assert_eq!(
        store.supersessions_by_successor(MemoryId(40)).unwrap(),
        vec![axon_memory::MemorySupersession {
            predecessor_memory_id: MemoryId(30),
            successor_memory_id: MemoryId(40),
            created_at_ms: 2_000,
        }]
    );
    let hits = orchestrator
        .hybrid_search(&MemoryHybridQuery {
            query_text: "服务 本地 远程".to_owned(),
            query_embedding: None,
            limit: 10,
            now_ms: 3_000,
            scope: None,
        })
        .unwrap();
    assert!(hits.iter().all(|hit| hit.record.record.id != MemoryId(30)));
    assert!(hits.iter().any(|hit| hit.record.record.id == MemoryId(40)));
}

#[test]
fn semantic_supersession_rejects_cross_scope_and_rolls_back() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = orchestrator(&temp);
    MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                candidate(5, "旧作用域事实"),
                50,
                scope("namespace:old"),
                Vec::new(),
                None,
                None,
                1_000,
            ),
        )
        .unwrap();

    let error = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(
                candidate(6, "跨作用域新事实"),
                60,
                scope("namespace:new"),
                vec![MemoryId(50)],
                None,
                None,
                2_000,
            ),
        )
        .expect_err("跨作用域替代必须整体拒绝");

    assert!(format!("{error:?}").contains("不能跨越作用域"));
    let store = orchestrator.sqlite_store().unwrap();
    assert!(store.get_record(MemoryId(60)).unwrap().is_none());
    assert!(
        store
            .get_record(MemoryId(50))
            .unwrap()
            .unwrap()
            .archived_at_ms
            .is_none()
    );
}

fn orchestrator(temp: &axon_test_support::TestTempDir) -> MemoryOrchestrator {
    MemoryOrchestrator::init(
        MemoryConfig::from_config_dir(temp.path(), true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap(),
    )
    .unwrap()
}

fn input(
    candidate: MemoryCandidate,
    memory_id: u64,
    scope: MemoryWriteScope,
    supersedes_memory_ids: Vec<MemoryId>,
    embedding: Option<Vec<f32>>,
    embedding_model_ref: Option<String>,
    now_ms: u64,
) -> MemoryWriteTriggerInput {
    MemoryWriteTriggerInput {
        memory_kind: axon_domain::MemoryKind::from_candidate_kind(candidate.candidate_kind),
        candidate,
        memory_id: MemoryId(memory_id),
        supersedes_memory_ids,
        scope,
        now_ms,
        ttl_expires_at_ms: None,
        importance_basis_points: MEMORY_WRITE_TRIGGER_DEFAULT_IMPORTANCE_BASIS_POINTS,
        embedding,
        embedding_model_ref,
    }
}

fn scope(namespace_ref: &str) -> MemoryWriteScope {
    MemoryWriteScope {
        namespace_ref: namespace_ref.to_owned(),
        repo_ref: Some("repo:test".to_owned()),
        user_ref: None,
    }
}

fn candidate(id: u64, summary: &str) -> MemoryCandidate {
    MemoryCandidate {
        id: MemoryCandidateId(id),
        source_trace: Some(MemorySourceTrace {
            origin: MemorySourceOrigin::SkillReflection,
            session_id: Some(SessionId(7)),
            cell_id: Some(CellId(id)),
            summary: summary.to_owned(),
            evidence_ref: Some(format!("test://memory-write/{id}")),
        }),
        candidate_kind: MemoryCandidateKind::ProceduralMemory,
        content_ref: format!("memory-ref:test-{id}"),
        confidence_basis_points: 9_000,
        sensitivity: MemorySensitivity::Internal,
        proposed_retention: MemoryRetentionPolicy::LongTerm,
        verification: MemoryVerification::AgentVerified,
    }
}

fn valid_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION];
    embedding[0] = 1.0;
    embedding
}
