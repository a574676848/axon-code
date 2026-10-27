use axon_agent_runtime::{
    ExecutionEvidenceIndex, SemanticMemoryCandidateType, SemanticSalienceAssessment,
    SemanticSalienceSignals, TurnFinalStatus, TurnOutcomeEnvelope,
};

use super::{
    completed_semantic_assessment, parse_semantic_assessment, semantic_salience_output_schema,
};

/// Provider 兼容层不接受数组 schema 上的 `uniqueItems`，去重约束由运行时校验承担。
#[test]
fn output_schema_omits_provider_unsupported_unique_items() {
    let schema = semantic_salience_output_schema();
    let properties = &schema["properties"];

    assert!(properties["candidateTypes"].get("uniqueItems").is_none());
    assert!(properties["evidenceRefs"].get("uniqueItems").is_none());
}

#[test]
fn invalid_json_is_conservatively_rejected() {
    let envelope = envelope();
    assert!(parse_semantic_assessment("not-json", &envelope).is_none());
    let outcome = completed_semantic_assessment("not-json", &envelope);
    assert!(outcome.completed);
    assert!(outcome.rejected);
}

#[test]
fn forged_evidence_ref_is_conservatively_rejected() {
    let raw = serde_json::to_string(&SemanticSalienceAssessment {
        schema_version: axon_agent_runtime::SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: true,
        candidate_types: vec![SemanticMemoryCandidateType::Preference],
        signals: SemanticSalienceSignals {
            stability: 9_000,
            future_utility: 9_000,
            scope_confidence: 9_000,
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: vec!["receipt:forged".to_owned()],
        reason: "用户表达了长期偏好".to_owned(),
        user_affect: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    })
    .unwrap();

    assert!(parse_semantic_assessment(&raw, &envelope()).is_none());
    let outcome = completed_semantic_assessment(&raw, &envelope());
    assert!(outcome.completed);
    assert!(outcome.rejected);
}

fn envelope() -> TurnOutcomeEnvelope {
    TurnOutcomeEnvelope {
        conversation_id: "conversation-1".to_owned(),
        turn_id: "turn-1".to_owned(),
        agent_path: "/root".to_owned(),
        user_intent_summary: "使用中文".to_owned(),
        assistant_outcome_summary: "已处理".to_owned(),
        user_message_ref: "message:user:1".to_owned(),
        execution_evidence: ExecutionEvidenceIndex::default(),
        selected_skill: None,
        final_status: TurnFinalStatus::Completed,
    }
}
