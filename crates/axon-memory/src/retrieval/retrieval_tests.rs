use axon_domain::{MemoryId, MemoryRecord, MemoryState};

use super::{
    Candidate, MemoryHybridScope, MemorySqliteRecord, MergeHitInput, candidate_to_hit, merge_hit,
    record_matches_scope,
};

/// 文本向量与图像向量命中同一条记录时，名次与距离都取更优的一路。
#[test]
fn vector_rank_and_distance_keep_the_better_path() {
    let record = MemorySqliteRecord {
        memory_kind: axon_domain::MemoryKind::Knowledge,
        record: MemoryRecord {
            id: MemoryId(1),
            candidate_id: axon_domain::MemoryCandidateId(1),
            state: MemoryState::Strong,
            content_ref: Some("memory://same".to_owned()),
            source_trace: None,
            tombstone: None,
        },
        namespace_ref: Some("workspace:current".to_owned()),
        repo_ref: None,
        user_ref: None,
        created_at_ms: 1,
        updated_at_ms: 1,
        last_active_at_ms: 1,
        ttl_expires_at_ms: None,
        confidence_basis_points: 8_000,
        importance_basis_points: 8_000,
        vector_id: None,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    };
    let mut candidates = std::collections::BTreeMap::new();
    for (rank, distance) in [(5usize, 0.5f32), (2, 0.2)] {
        assert!(merge_hit(
            &mut candidates,
            MergeHitInput {
                record: record.clone(),
                text_score: None,
                vector_distance: Some(distance),
                text_rank: None,
                vector_rank: Some(rank),
                now_ms: 100,
                scope: None,
            },
        ));
    }
    let candidate = candidates.get(&MemoryId(1)).expect("candidate 应存在");
    assert_eq!(candidate.vector_rank, Some(2));
    assert_eq!(candidate.vector_distance, Some(0.2));
}

#[test]
fn hybrid_scope_mismatch_fails_closed() {
    let record = MemorySqliteRecord {
        memory_kind: axon_domain::MemoryKind::Knowledge,
        record: MemoryRecord {
            id: MemoryId(1),
            candidate_id: axon_domain::MemoryCandidateId(1),
            state: MemoryState::Strong,
            content_ref: Some("memory://workspace".to_owned()),
            source_trace: None,
            tombstone: None,
        },
        namespace_ref: Some("workspace:other".to_owned()),
        repo_ref: None,
        user_ref: None,
        created_at_ms: 1,
        updated_at_ms: 1,
        last_active_at_ms: 1,
        ttl_expires_at_ms: None,
        confidence_basis_points: 8_000,
        importance_basis_points: 8_000,
        vector_id: None,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    };

    let scope = MemoryHybridScope {
        user_ref: None,
        namespace_ref: "workspace:current".to_owned(),
    };
    assert!(!record_matches_scope(&record, Some(&scope)));
}

fn fixture_record(id: MemoryId, confidence: u16, importance: u16) -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: axon_domain::MemoryKind::Knowledge,
        record: MemoryRecord {
            id,
            candidate_id: axon_domain::MemoryCandidateId(1),
            state: MemoryState::Strong,
            content_ref: Some("memory://workspace".to_owned()),
            source_trace: None,
            tombstone: None,
        },
        namespace_ref: Some("workspace:current".to_owned()),
        repo_ref: None,
        user_ref: None,
        created_at_ms: 1,
        updated_at_ms: 1,
        last_active_at_ms: 1,
        ttl_expires_at_ms: None,
        confidence_basis_points: confidence,
        importance_basis_points: importance,
        vector_id: None,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

#[test]
fn core_fallback_candidate_is_weighted_by_governance() {
    let hit = candidate_to_hit(Candidate {
        record: fixture_record(MemoryId(1), 8_500, 8_000),
        text_score: Some(8_250.0),
        vector_distance: None,
        text_rank: None,
        vector_rank: None,
    });

    assert_eq!(hit.explain.weights, "core_fallback");
    assert_eq!(hit.explain.text_component_basis_points, 0);
    assert_eq!(hit.explain.vector_component_basis_points, 0);
    assert_eq!(hit.explain.governance_component_basis_points, 825);
    assert_eq!(hit.score_basis_points, 825);
}

#[test]
fn core_fallback_never_outranks_primary_rank_hit() {
    let fallback = candidate_to_hit(Candidate {
        record: fixture_record(MemoryId(1), 8_500, 8_000),
        text_score: Some(10_000.0),
        vector_distance: None,
        text_rank: None,
        vector_rank: None,
    });
    let primary_text = candidate_to_hit(Candidate {
        record: fixture_record(MemoryId(2), 8_500, 8_000),
        text_score: Some(1.0),
        vector_distance: None,
        text_rank: Some(1),
        vector_rank: None,
    });
    let primary_vector = candidate_to_hit(Candidate {
        record: fixture_record(MemoryId(3), 8_500, 8_000),
        text_score: None,
        vector_distance: Some(0.1),
        text_rank: None,
        vector_rank: Some(1),
    });

    assert!(
        fallback.score_basis_points < primary_text.score_basis_points,
        "兜底分 {} 不得压过文本 rank=1 命中 {}",
        fallback.score_basis_points,
        primary_text.score_basis_points
    );
    assert!(
        fallback.score_basis_points < primary_vector.score_basis_points,
        "兜底分 {} 不得压过向量 rank=1 命中 {}",
        fallback.score_basis_points,
        primary_vector.score_basis_points
    );
}
