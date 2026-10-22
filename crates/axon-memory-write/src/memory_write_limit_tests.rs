use axon_domain::{
    CellId, MemoryAdmissionGate, MemoryAdmissionPolicy, MemoryCandidate, MemoryCandidateId,
    MemoryCandidateKind, MemoryId, MemoryRetentionPolicy, MemorySensitivity, MemorySourceOrigin,
    MemorySourceTrace, MemoryVerification, SessionId,
};
use axon_memory::{MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig, MemoryOrchestrator};

use super::{
    MEMORY_WRITE_TRIGGER_DEFAULT_IMPORTANCE_BASIS_POINTS, MemoryWriteTriggerInput,
    MemoryWriteTriggerRuntime, memory_scope_refs,
};

#[test]
fn write_deduplicates_records_older_than_legacy_scan_window() {
    const LEGACY_DEDUPE_SCAN_LIMIT: u64 = 512;
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = MemoryOrchestrator::init(
        MemoryConfig::from_config_dir(temp.path(), true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap(),
    )
    .unwrap();
    let scope = memory_scope_refs(temp.path());

    seed_active_records(&orchestrator, &scope, LEGACY_DEDUPE_SCAN_LIMIT + 1);
    let report = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(candidate(1), LEGACY_DEDUPE_SCAN_LIMIT + 2, 10_000, scope),
        )
        .unwrap();

    assert!(report.merged_existing);
    assert_eq!(report.duplicate_memory_id, Some(MemoryId(1)));
}

#[test]
fn retention_policy_does_not_create_an_implicit_ttl() {
    let temp = axon_test_support::test_temp_dir();
    let orchestrator = MemoryOrchestrator::init(
        MemoryConfig::from_config_dir(temp.path(), true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap(),
    )
    .unwrap();
    let mut candidate = candidate(1);
    candidate.proposed_retention = MemoryRetentionPolicy::Session;

    let report = MemoryWriteTriggerRuntime
        .trigger(
            &orchestrator,
            &input(candidate, 1, 1_000, memory_scope_refs(temp.path())),
        )
        .unwrap();

    assert_eq!(report.record.unwrap().id, MemoryId(1));
    let stored = orchestrator
        .sqlite_store()
        .unwrap()
        .find_active_by_content_ref("memory-ref:test-1")
        .unwrap()
        .unwrap();
    assert_eq!(stored.ttl_expires_at_ms, None);
}

fn seed_active_records(
    orchestrator: &MemoryOrchestrator,
    scope: &super::MemoryWriteScope,
    count: u64,
) {
    let store = orchestrator.sqlite_store().unwrap();
    for id in 1..=count {
        let input = input(candidate(id), id, id, scope.clone());
        let admission = MemoryAdmissionGate::evaluate_candidate(
            &input.candidate,
            MemoryAdmissionPolicy::default(),
        );
        let record =
            MemoryAdmissionGate::record_from_decision(input.memory_id, &input.candidate, admission)
                .expect("测试候选必须可写入");
        store
            .upsert_record(&super::sqlite_record(&input, &record, None, None))
            .unwrap();
    }
}

fn input(
    candidate: MemoryCandidate,
    memory_id: u64,
    now_ms: u64,
    scope: super::MemoryWriteScope,
) -> MemoryWriteTriggerInput {
    MemoryWriteTriggerInput {
        memory_kind: axon_domain::MemoryKind::from_candidate_kind(candidate.candidate_kind),
        candidate,
        memory_id: MemoryId(memory_id),
        supersedes_memory_ids: Vec::new(),
        scope,
        now_ms,
        ttl_expires_at_ms: None,
        importance_basis_points: MEMORY_WRITE_TRIGGER_DEFAULT_IMPORTANCE_BASIS_POINTS,
        embedding: None,
        embedding_model_ref: None,
    }
}

fn candidate(id: u64) -> MemoryCandidate {
    MemoryCandidate {
        id: MemoryCandidateId(id),
        source_trace: Some(MemorySourceTrace {
            origin: MemorySourceOrigin::SkillReflection,
            session_id: Some(SessionId(7)),
            cell_id: Some(CellId(id)),
            summary: format!("写入去重测试 {id}"),
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
