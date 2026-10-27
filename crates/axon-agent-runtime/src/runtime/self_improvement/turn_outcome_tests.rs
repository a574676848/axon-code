use super::*;

#[test]
fn stable_user_preference_qualifies_without_tool_execution() {
    let envelope = envelope(Vec::new());
    let assessment = SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: true,
        candidate_types: vec![SemanticMemoryCandidateType::Preference],
        signals: SemanticSalienceSignals {
            stability: 9_000,
            future_utility: 9_000,
            scope_confidence: 9_000,
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: vec![envelope.user_message_ref.clone()],
        reason: "用户明确表达了稳定偏好".to_owned(),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    };

    assert!(assessment.qualifies_for_reflection(&envelope));
}

#[test]
fn model_claim_without_runtime_receipt_cannot_qualify_experience() {
    let envelope = envelope(Vec::new());
    let assessment = experience_assessment(&envelope, envelope.user_message_ref.clone());

    assert!(assessment.is_valid_for(&envelope));
    assert!(!assessment.qualifies_for_reflection(&envelope));
}

#[test]
fn verified_tool_receipt_allows_high_value_experience() {
    let evidence_ref = "receipt:1".to_owned();
    let envelope = envelope(vec![ExecutionEvidenceRecord {
        evidence_ref: evidence_ref.clone(),
        tool_name: "builtin.shell_command".to_owned(),
        status: ExecutionEvidenceStatus::Verified,
        kind: ExecutionEvidenceKind::Validation,
        runtime_verified: true,
        verification_refs: Vec::new(),
    }]);
    let assessment = experience_assessment(&envelope, evidence_ref);

    assert!(assessment.qualifies_for_reflection(&envelope));
}

#[test]
fn forged_evidence_reference_invalidates_assessment() {
    let envelope = envelope(Vec::new());
    let mut assessment = experience_assessment(&envelope, "receipt:missing".to_owned());
    assessment.candidate_types = vec![SemanticMemoryCandidateType::Preference];

    assert!(!assessment.is_valid_for(&envelope));
}

#[test]
fn conservative_skip_requires_no_candidate_or_evidence() {
    let envelope = envelope(Vec::new());
    let assessment = SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: false,
        candidate_types: Vec::new(),
        signals: SemanticSalienceSignals::default(),
        evidence_refs: Vec::new(),
        reason: "没有跨会话价值".to_owned(),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    };

    assert!(assessment.is_valid_for(&envelope));
    assert!(!assessment.qualifies_for_reflection(&envelope));
}

/// Provider 兼容层不接受 `uniqueItems`，重复项必须由运行时保守拒绝。
#[test]
fn duplicate_candidate_or_evidence_invalidates_assessment() {
    let evidence_ref = "receipt:1";
    let envelope = envelope(vec![ExecutionEvidenceRecord {
        evidence_ref: evidence_ref.to_owned(),
        tool_name: "builtin.shell".to_owned(),
        status: ExecutionEvidenceStatus::Verified,
        kind: ExecutionEvidenceKind::Validation,
        runtime_verified: true,
        verification_refs: Vec::new(),
    }]);

    let mut duplicated_candidate = experience_assessment(&envelope, evidence_ref.to_owned());
    duplicated_candidate.candidate_types = vec![
        SemanticMemoryCandidateType::Preference,
        SemanticMemoryCandidateType::Preference,
    ];
    assert!(!duplicated_candidate.is_valid_for(&envelope));

    let mut duplicated_evidence = experience_assessment(&envelope, evidence_ref.to_owned());
    duplicated_evidence.evidence_refs = vec![evidence_ref.to_owned(), evidence_ref.to_owned()];
    assert!(!duplicated_evidence.is_valid_for(&envelope));
}

fn experience_assessment(
    envelope: &TurnOutcomeEnvelope,
    evidence_ref: String,
) -> SemanticSalienceAssessment {
    SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: true,
        candidate_types: vec![SemanticMemoryCandidateType::Experience],
        signals: SemanticSalienceSignals {
            future_utility: 9_000,
            verification_strength: 9_000,
            scope_confidence: 9_000,
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: vec![evidence_ref],
        reason: format!("{} 形成了可复用经验", envelope.turn_id),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    }
}

fn envelope(records: Vec<ExecutionEvidenceRecord>) -> TurnOutcomeEnvelope {
    TurnOutcomeEnvelope {
        conversation_id: "conversation-1".to_owned(),
        turn_id: "turn-1".to_owned(),
        agent_path: "/root".to_owned(),
        user_intent_summary: "记住我的稳定偏好".to_owned(),
        assistant_outcome_summary: "已处理".to_owned(),
        user_message_ref: "message:user:1".to_owned(),
        execution_evidence: ExecutionEvidenceIndex { records },
        selected_skill: None,
        final_status: TurnFinalStatus::Completed,
    }
}

#[test]
fn style_only_never_qualifies_for_memory_reflection() {
    let envelope = envelope(Vec::new());
    let assessment = SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: true,
        candidate_types: vec![SemanticMemoryCandidateType::Style],
        signals: SemanticSalienceSignals {
            stability: 9_000,
            future_utility: 9_000,
            scope_confidence: 9_000,
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: vec![envelope.user_message_ref.clone()],
        reason: "用户要求少寒暄".to_owned(),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: Some("少用寒暄，结论先行".to_owned()),
    };

    assert!(
        !assessment.qualifies_for_reflection(&envelope),
        "风格候选必须由人格系统承接，不得触发 Memory 反思"
    );
}

#[test]
fn style_mixed_with_preference_still_qualifies_through_the_preference() {
    let envelope = envelope(Vec::new());
    let mut assessment = SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: true,
        candidate_types: vec![
            SemanticMemoryCandidateType::Style,
            SemanticMemoryCandidateType::Preference,
        ],
        signals: SemanticSalienceSignals {
            stability: 9_000,
            future_utility: 9_000,
            scope_confidence: 9_000,
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: vec![envelope.user_message_ref.clone()],
        reason: "用户同时给出了风格与行为偏好".to_owned(),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: Some("少用寒暄".to_owned()),
    };
    assessment.style_candidate = None;

    assert!(
        assessment.qualifies_for_reflection(&envelope),
        "混合候选里只要含可反思类型就该触发，只是风格自身不参与判定"
    );
}
