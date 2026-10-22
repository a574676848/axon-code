use crate::{
    CellId, MemoryAdmissionGate, MemoryAdmissionPolicy, MemoryAdmissionReason, MemoryCandidate,
    MemoryCandidateId, MemoryCandidateKind, MemoryKind, MemoryRetentionPolicy, MemorySensitivity,
    MemorySourceOrigin, MemorySourceTrace, MemoryVerification, MemoryWriteDecision, SessionId,
};

#[test]
fn memory_kind_codes_round_trip_without_aliases() {
    let kinds = [
        MemoryKind::Profile,
        MemoryKind::Preference,
        MemoryKind::Experience,
        MemoryKind::Knowledge,
        MemoryKind::Procedure,
    ];
    for kind in kinds {
        assert_eq!(MemoryKind::from_code(kind.as_str()), Some(kind));
    }
    assert_eq!(MemoryKind::from_code("agentKnowledge"), None);
}

#[test]
fn agent_verified_strong_candidate_writes_without_user_review() {
    let decision = MemoryAdmissionGate::evaluate_candidate(
        &candidate(
            MemorySourceOrigin::SkillReflection,
            MemoryVerification::AgentVerified,
        ),
        MemoryAdmissionPolicy::default(),
    );

    assert_eq!(decision.decision, MemoryWriteDecision::WriteStrong);
    assert_eq!(decision.reason, MemoryAdmissionReason::StrongAllowed);
    assert!(decision.strong_memory_allowed);
}

#[test]
fn unverified_strong_candidate_requires_user_review() {
    let decision = MemoryAdmissionGate::evaluate_candidate(
        &candidate(
            MemorySourceOrigin::SkillReflection,
            MemoryVerification::Unverified,
        ),
        MemoryAdmissionPolicy::default(),
    );

    assert_eq!(decision.decision, MemoryWriteDecision::NeedsUserReview);
    assert_eq!(decision.reason, MemoryAdmissionReason::LocalReviewRequired);
    assert!(!decision.strong_memory_allowed);
}

#[test]
fn remote_upload_still_requires_user_review() {
    let decision = MemoryAdmissionGate::evaluate_candidate(
        &candidate(
            MemorySourceOrigin::RemoteUpload,
            MemoryVerification::AgentVerified,
        ),
        MemoryAdmissionPolicy::default(),
    );

    assert_eq!(decision.decision, MemoryWriteDecision::NeedsUserReview);
    assert_eq!(
        decision.reason,
        MemoryAdmissionReason::RemoteSourceRequiresReview
    );
}

#[test]
fn secret_content_is_rejected_and_secret_adjacent_is_quarantined() {
    let mut secret = candidate(
        MemorySourceOrigin::SkillReflection,
        MemoryVerification::AgentVerified,
    );
    secret.sensitivity = MemorySensitivity::Secret;
    let secret_decision =
        MemoryAdmissionGate::evaluate_candidate(&secret, MemoryAdmissionPolicy::default());

    let mut adjacent = candidate(
        MemorySourceOrigin::SkillReflection,
        MemoryVerification::AgentVerified,
    );
    adjacent.sensitivity = MemorySensitivity::SecretAdjacent;
    let adjacent_decision =
        MemoryAdmissionGate::evaluate_candidate(&adjacent, MemoryAdmissionPolicy::default());

    assert_eq!(secret_decision.decision, MemoryWriteDecision::Reject);
    assert_eq!(
        secret_decision.reason,
        MemoryAdmissionReason::SecretRejected
    );
    assert_eq!(adjacent_decision.decision, MemoryWriteDecision::Quarantine);
    assert_eq!(
        adjacent_decision.reason,
        MemoryAdmissionReason::SecretAdjacentQuarantined
    );
}

fn candidate(origin: MemorySourceOrigin, verification: MemoryVerification) -> MemoryCandidate {
    MemoryCandidate {
        id: MemoryCandidateId(1),
        source_trace: Some(MemorySourceTrace {
            origin,
            session_id: Some(SessionId(7)),
            cell_id: Some(CellId(9)),
            summary: "闭环验证通过".to_owned(),
            evidence_ref: Some("runtime-cell:7:9".to_owned()),
        }),
        candidate_kind: MemoryCandidateKind::ProceduralMemory,
        content_ref: "memory-ref:verified".to_owned(),
        confidence_basis_points: 9_000,
        sensitivity: MemorySensitivity::Internal,
        proposed_retention: MemoryRetentionPolicy::Project,
        verification,
    }
}
