use super::*;

use axon_domain::{
    MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin, MemorySourceTrace,
    MemoryState,
};

use crate::MemorySqliteRecord;

const DAY: u64 = 86_400_000;

fn record(
    id: u64,
    kind: MemoryKind,
    state: MemoryState,
    confidence_bp: u16,
    importance_bp: u16,
    last_active_at_ms: u64,
    archived_at_ms: Option<u64>,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        record: MemoryRecord {
            id: MemoryId(id),
            candidate_id: MemoryCandidateId(1),
            state,
            content_ref: Some(format!("memory-tool:{id}")),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::AgentKnowledge,
                session_id: None,
                cell_id: None,
                summary: format!("记录 {id}"),
                evidence_ref: None,
            }),
            tombstone: None,
        },
        memory_kind: kind,
        namespace_ref: Some("memory-scope:local".to_owned()),
        repo_ref: None,
        user_ref: None,
        created_at_ms: 1,
        updated_at_ms: last_active_at_ms,
        last_active_at_ms,
        ttl_expires_at_ms: None,
        confidence_basis_points: confidence_bp,
        importance_basis_points: importance_bp,
        vector_id: None,
        archived_at_ms,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

fn fresh_knowledge(now: u64) -> MemorySqliteRecord {
    record(
        1,
        MemoryKind::Knowledge,
        MemoryState::Strong,
        8_000,
        8_000,
        now,
        None,
    )
}

#[test]
fn full_score_record_keeps_top_rank_when_fresh() {
    let now = 1_000 * DAY;
    let record = fresh_knowledge(now);
    assert_eq!(aging_score_basis_points(&record, now), 6_400);
    assert_eq!(injection_rank(&record, now), (6_400, now, 1));
    assert!(!should_archive(&record, now));
}

#[test]
fn decay_reaches_archive_threshold_after_about_63_days() {
    let now = 1_000 * DAY;
    let record = fresh_knowledge(now);
    // 半衰期 30 天，满分 6400 跌到 1500 需要约 63 天。
    assert!(decayed_score_basis_points(&record, now + 62 * DAY) > 1_500);
    assert!(decayed_score_basis_points(&record, now + 64 * DAY) < 1_500);
    assert!(should_archive(&record, now + 64 * DAY));
}

#[test]
fn archived_or_tombstoned_record_is_not_recallable() {
    let now = 1_000 * DAY;
    let archived = record(
        2,
        MemoryKind::Knowledge,
        MemoryState::Strong,
        8_000,
        8_000,
        now,
        Some(now),
    );
    let tombstoned = record(
        3,
        MemoryKind::Knowledge,
        MemoryState::Tombstone,
        8_000,
        8_000,
        now,
        None,
    );
    assert!(!is_active_recallable(&archived));
    assert!(!is_active_recallable(&tombstoned));
    // 已归档记录不再参与治理记账。
    assert!(!should_archive(&archived, now + 400 * DAY));
}

#[test]
fn record_without_content_ref_is_not_recallable() {
    let now = 1_000 * DAY;
    let mut record = fresh_knowledge(now);
    record.record.content_ref = Some("   ".to_owned());
    assert!(!is_active_recallable(&record));
}

#[test]
fn stale_procedure_is_demoted_below_equal_age_knowledge() {
    let now = 1_000 * DAY;
    let updated = now - 200 * DAY;
    let procedure = record(
        4,
        MemoryKind::Procedure,
        MemoryState::Strong,
        8_000,
        8_000,
        updated,
        None,
    );
    let knowledge = record(
        5,
        MemoryKind::Knowledge,
        MemoryState::Strong,
        8_000,
        8_000,
        updated,
        None,
    );
    let procedure_score = aging_score_basis_points(&procedure, now);
    let knowledge_score = aging_score_basis_points(&knowledge, now);
    assert_eq!(procedure_score, knowledge_score / 2);
    assert!(injection_rank(&procedure, now) < injection_rank(&knowledge, now));
}

#[test]
fn expired_procedure_scores_zero_but_stays_recallable() {
    let now = 1_000 * DAY;
    let expired = record(
        6,
        MemoryKind::Procedure,
        MemoryState::Strong,
        8_000,
        8_000,
        now - 400 * DAY,
        None,
    );
    assert_eq!(aging_score_basis_points(&expired, now), 0);
    assert!(is_active_recallable(&expired), "过期不等于不可召回");
}
