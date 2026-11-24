use super::{
    SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS, SelfIterationHookKind, SelfIterationInput,
    SelfIterationRuntime, SelfIterationSkipReason, SelfIterationSubagentResult,
    SelfIterationSubagentStatus,
};
use crate::{
    ExecutionEvidenceIndex, ExecutionEvidenceKind, ExecutionEvidenceRecord,
    ExecutionEvidenceStatus, SEMANTIC_SALIENCE_SCHEMA_VERSION, SelectedSkillExecution,
    SemanticMemoryCandidateType, SemanticSalienceAssessment, SemanticSalienceSignals,
    TurnFinalStatus, TurnOutcomeEnvelope,
};
use axon_domain::SessionId;

#[test]
fn self_iteration_plans_memory_update_for_completed_root_task() {
    let plan = SelfIterationRuntime.plan(&SelfIterationInput {
        parent_total_cost_micros: 1_000,
        ..base_input()
    });

    let hook = plan
        .hooks
        .iter()
        .find(|hook| hook.kind == SelfIterationHookKind::MemoryUpdate)
        .unwrap();
    assert!(hook.task_message.contains("builtin.memory_search"));
    // 新契约：子代理只交候选，写入由运行时统一执行，因此提示词里必须出现
    // 「不要调用 memory_write」与候选 JSON 的形状，而不是旧的 supersedes 参数名。
    assert!(hook.task_message.contains("不要调用 builtin.memory_write"));
    assert!(hook.task_message.contains("\"candidates\""));
    assert!(hook.task_message.contains("\"turnOutcome\""));
    assert!(!hook.inherits_model_tools_history);
    assert_eq!(
        hook.max_runtime_ms,
        Some(SELF_ITERATION_MEMORY_UPDATE_MAX_RUNTIME_MS)
    );
    assert!(plan.complete_after_hooks);
    assert!(!plan.remote_trigger_allowed);
}

#[test]
fn self_iteration_blocks_recursive_subagent_reflection() {
    let subagent = SelfIterationRuntime.plan(&SelfIterationInput {
        is_subagent: true,
        ..base_input()
    });

    assert!(subagent.hooks.is_empty());
    assert!(subagent.skipped.iter().any(|skip| {
        skip.hook == SelfIterationHookKind::MemoryUpdate
            && skip.reason == SelfIterationSkipReason::SubagentRecursionBlocked
    }));
}

#[test]
fn self_iteration_skips_memory_reflection_for_low_salience_turn() {
    let mut input = base_input();
    input.semantic_assessment = Some(SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect: false,
        candidate_types: Vec::new(),
        signals: SemanticSalienceSignals::default(),
        evidence_refs: Vec::new(),
        reason: "普通任务没有长期价值".to_owned(),
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    });
    let plan = SelfIterationRuntime.plan(&input);

    assert!(plan.hooks.is_empty());
    assert!(plan.skipped.iter().any(|skip| {
        skip.hook == SelfIterationHookKind::MemoryUpdate
            && skip.reason == SelfIterationSkipReason::SemanticSignalBelowThreshold
    }));
}

#[test]
fn self_iteration_merges_subagent_cost_and_surfaces_memory_write_only() {
    let plan = SelfIterationRuntime.plan(&SelfIterationInput {
        parent_total_cost_micros: 1_000,
        subagent_results: vec![SelfIterationSubagentResult {
            hook: SelfIterationHookKind::MemoryUpdate,
            label: "memory_update".to_owned(),
            cost_micros: 100,
            status: SelfIterationSubagentStatus::CompletedWithWrite,
            bounded_summary: "memory updated".to_owned(),
        }],
        ..base_input()
    });

    assert_eq!(plan.cost_merge.subagent_cost_micros, 100);
    assert_eq!(plan.cost_merge.merged_total_cost_micros, 1_100);
    assert!(!plan.interrupt_should_propagate);
}

#[test]
fn self_iteration_propagates_interrupt_but_not_nonfatal_failures() {
    let plan = SelfIterationRuntime.plan(&SelfIterationInput {
        subagent_results: vec![SelfIterationSubagentResult {
            hook: SelfIterationHookKind::MemoryUpdate,
            label: "memory_update".to_owned(),
            cost_micros: 0,
            status: SelfIterationSubagentStatus::Interrupted,
            bounded_summary: "interrupted".to_owned(),
        }],
        ..base_input()
    });

    assert!(plan.interrupt_should_propagate);
}

#[test]
fn self_iteration_bridges_hooks_into_subagent_delegation_plan() {
    let plan = SelfIterationRuntime
        .plan_with_delegation(SessionId(7), false, &base_input())
        .unwrap();

    let delegation_plan = plan.delegation_plan.as_ref().unwrap();
    assert_eq!(delegation_plan.entries.len(), 1);
    assert_eq!(delegation_plan.entries[0].label, "memory_update");
    assert!(!plan.remote_trigger_allowed);
}

fn base_input() -> SelfIterationInput {
    let turn_outcome = verified_outcome();
    SelfIterationInput {
        is_subagent: false,
        semantic_assessment: Some(SemanticSalienceAssessment {
            schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
            should_reflect: true,
            candidate_types: vec![SemanticMemoryCandidateType::Experience],
            signals: SemanticSalienceSignals {
                future_utility: 9_000,
                verification_strength: 9_000,
                scope_confidence: 9_000,
                ..SemanticSalienceSignals::default()
            },
            evidence_refs: vec!["receipt:test".to_owned()],
            reason: "已验证的执行经验可跨会话复用".to_owned(),
            user_affect: None,
            user_fatigue: None,
            persona_drift_bp: None,
            drift_reason: None,
            style_candidate: None,
        }),
        turn_outcome,
        reflection_cards: Vec::new(),
        parent_total_cost_micros: 0,
        subagent_results: Vec::new(),
    }
}

fn verified_outcome() -> TurnOutcomeEnvelope {
    TurnOutcomeEnvelope {
        conversation_id: "conversation-1".to_owned(),
        turn_id: "turn-1".to_owned(),
        agent_path: "/root".to_owned(),
        user_intent_summary: "修复并验证问题".to_owned(),
        assistant_outcome_summary: "修复完成且测试通过".to_owned(),
        user_message_ref: "message:user:1".to_owned(),
        execution_evidence: ExecutionEvidenceIndex {
            records: vec![ExecutionEvidenceRecord {
                evidence_ref: "receipt:test".to_owned(),
                tool_name: "builtin.shell_command".to_owned(),
                status: ExecutionEvidenceStatus::Verified,
                kind: ExecutionEvidenceKind::Validation,
                runtime_verified: true,
                verification_refs: Vec::new(),
            }],
        },
        selected_skill: Some(SelectedSkillExecution {
            skill_name: "release-notes".to_owned(),
            execution_verified: true,
            evidence_refs: vec!["receipt:skill".to_owned(), "receipt:test".to_owned()],
        }),
        final_status: TurnFinalStatus::Completed,
    }
}
