use axon_agent_runtime::{
    ExecutionEvidenceIndex, SemanticMemoryCandidateType, SemanticSalienceAssessment,
    SemanticSalienceSignals, TurnFinalStatus, TurnOutcomeEnvelope,
};

use crate::run_commands::run_semantic_salience::{
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
        user_fatigue: None,
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

#[test]
fn reflection_cards_carry_the_folded_v3_specificity() {
    let envelope = envelope();
    let trace = axon_agent_runtime::TurnTrace::from_outcome(
        &envelope,
        "trace:conversation-1:0",
        1_000,
        axon_agent_runtime::ExplicitSignals::default(),
    );
    let assessment = SemanticSalienceAssessment {
        schema_version: axon_agent_runtime::SEMANTIC_SALIENCE_ACCEPTED_MIN_VERSION,
        should_reflect: true,
        candidate_types: vec![SemanticMemoryCandidateType::Procedure],
        signals: SemanticSalienceSignals {
            verification_strength: 9_000,
            future_utility: 8_000,
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: Vec::new(),
        reason: "已验证的执行流程".to_owned(),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    };

    let cards = super::reflection_cards(
        &trace,
        Some(&assessment),
        std::path::Path::new("E:/zbg/axon-code"),
    );

    assert_eq!(cards.len(), 1, "无回执的轮次也要给一张索引卡");
    assert_eq!(cards[0].trace_id, "trace:conversation-1:0");
    assert_eq!(
        cards[0].signals.specificity,
        axon_agent_runtime::EXECUTION_SPECIFICITY_MIN_BP,
        "v3 输出没有 specificity 字段，必须折中到门槛，不能落 0"
    );
    assert_eq!(cards[0].location.workspace_kind, "project");
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
