use axon_domain::{
    AudienceContext, AudienceKind, CellId, DecisionEffect, DecisionId, DecisionRecord,
    DisclosurePolicy, ExecutionId, GoalId, GoalPlan, GoalRecord, GoalStatus, MemoryCandidateId,
    MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin, MemorySourceTrace, MemoryState, PaneId,
    ResultId, ResultRecord, ScopeRef, SessionId, TokenBudget,
};
use axon_memory::{MemoryHybridExplain, MemoryHybridHit, MemorySqliteRecord};

use crate::{
    AgentContextRuntime, AgentContextSectionKind, AgentContextSnapshotInput,
    AgentMemoryRecallInput, CommandActivityContext, CommandActivityKind, ContextSectionDecision,
    LedgerContextProjection, RecentCommandState,
};

#[test]
fn memory_recall_projects_working_memory_before_long_term_layers() {
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(9),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: None,
        shell_history: vec![RecentCommandState::new("cargo test", Some(101))],
        command_activity: Some(CommandActivityContext {
            command: "cargo test".to_owned(),
            activity: CommandActivityKind::Running,
            output_tail: "test memory_layer failed".to_owned(),
            remote_full_context_allowed: false,
            remote_trigger_allowed: false,
        }),
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(2048).unwrap(),
        reserved_for_tools: TokenBudget::new(256).unwrap(),
    });

    let injected = AgentContextRuntime.inject_memory_recall(
        snapshot,
        &AgentMemoryRecallInput {
            hits: vec![
                memory_hit(20, MemorySourceOrigin::DebuggingExperience),
                memory_hit(21, MemorySourceOrigin::ExplicitUserPreference),
            ],
            code_symbol_hits: Vec::new(),
            audience: private_audience(),
        },
    );

    let memory = injected
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::MemoryRecall)
        .expect("Memory recall 应注入上下文");
    let memory_content = memory.content();
    let lines = memory_content.lines().collect::<Vec<_>>();

    assert!(lines[0].contains("layer=working_memory"));
    assert!(lines[0].contains("persisted=false"));
    assert!(memory_content.contains("layer=episodic_memory"));
    assert!(memory_content.contains("layer=semantic_memory"));
    assert!(!memory_content.contains("source summary raw text"));
    assert_eq!(lines.len(), 4);
}

#[test]
fn memory_recall_projects_all_hits_before_token_budgeting() {
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(10),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: None,
        shell_history: vec![RecentCommandState::new("cargo check", Some(101))],
        command_activity: Some(CommandActivityContext {
            command: "cargo check".to_owned(),
            activity: CommandActivityKind::Running,
            output_tail: "Compiling axon".to_owned(),
            remote_full_context_allowed: false,
            remote_trigger_allowed: false,
        }),
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(256).unwrap(),
        reserved_for_tools: TokenBudget::new(1).unwrap(),
    });

    let injected = AgentContextRuntime.inject_memory_recall(
        snapshot,
        &AgentMemoryRecallInput {
            hits: (30..94)
                .map(|id| memory_hit(id, MemorySourceOrigin::DebuggingExperience))
                .collect(),
            code_symbol_hits: Vec::new(),
            audience: private_audience(),
        },
    );

    let memory = injected
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::MemoryRecall)
        .expect("Memory recall 应注入上下文");

    let memory_content = memory.content();
    let working_memory_lines = memory_content
        .lines()
        .filter(|line| line.contains("layer=working_memory"))
        .count();
    assert_eq!(working_memory_lines, 2);
    assert_eq!(memory_content.lines().count(), working_memory_lines + 64);
    assert!(memory_content.contains("memory-ref:layer-93"));
    let memory_budget = injected
        .budget_plan
        .sections
        .iter()
        .find(|section| section.label == "memory_recall")
        .unwrap();
    assert!(matches!(
        memory_budget.decision,
        ContextSectionDecision::Summarize { .. }
    ));
    assert!(memory_budget.included_token_estimate < memory_budget.original_token_estimate);
}

#[test]
fn group_audience_filters_global_memory_before_injection() {
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(11),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: None,
        shell_history: Vec::new(),
        command_activity: None,
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(512).unwrap(),
        reserved_for_tools: TokenBudget::new(1).unwrap(),
    });
    let mut global = memory_hit(40, MemorySourceOrigin::ExplicitUserPreference);
    global.record.namespace_ref = Some(axon_memory::MEMORY_GLOBAL_NAMESPACE_REF.to_owned());
    global.record.repo_ref = None;
    let workspace = memory_hit(41, MemorySourceOrigin::DebuggingExperience);

    let injected = AgentContextRuntime.inject_memory_recall(
        snapshot,
        &AgentMemoryRecallInput {
            hits: vec![global, workspace],
            code_symbol_hits: Vec::new(),
            audience: AudienceContext::new(AudienceKind::Group, DisclosurePolicy::Workspace),
        },
    );
    let memory = injected
        .sections
        .iter()
        .find(|section| section.kind() == AgentContextSectionKind::MemoryRecall)
        .expect("群聊仍应保留 Workspace Memory");
    let content = memory.content();
    assert!(!content.contains("memory-ref:layer-40"));
    assert!(content.contains("memory-ref:layer-41"));
}

#[test]
fn ledger_context_precedes_memory_and_drops_invalid_decision() {
    let scope = ScopeRef::workspace("ledger-workspace").unwrap();
    let task = GoalRecord::new(
        GoalId::new("task-ledger").unwrap(),
        scope.clone(),
        "发布版本",
        GoalPlan {
            objective: "完成发布".to_owned(),
            steps: vec!["验证构建".to_owned()],
        },
        vec!["通过测试".to_owned()],
        1,
    )
    .unwrap();
    let mut active_task = task.clone();
    active_task.transition(GoalStatus::Active, 2).unwrap();
    let expired_decision = DecisionRecord {
        decision_id: DecisionId::new("decision-expired").unwrap(),
        scope: scope.clone(),
        actor: "operator".to_owned(),
        effect: DecisionEffect::Allow,
        revision: 1,
        expires_at_ms: Some(10),
        created_at_ms: 1,
    };
    let result = ResultRecord::success(
        ResultId::new("result-unread").unwrap(),
        active_task.goal_id.clone(),
        ExecutionId::new("execution-unread").unwrap(),
        "构建已完成",
        3,
    );
    let snapshot = AgentContextRuntime.snapshot(AgentContextSnapshotInput {
        pane_id: PaneId(12),
        cwd: "E:\\zbg\\axon-code".to_owned(),
        git_state: None,
        shell_history: Vec::new(),
        command_activity: None,
        apply_history: Vec::new(),
        chat_history: Vec::new(),
        total_limit: TokenBudget::new(4096).unwrap(),
        reserved_for_tools: TokenBudget::new(1).unwrap(),
    });
    let snapshot = AgentContextRuntime.inject_ledger_context(
        snapshot,
        &LedgerContextProjection {
            scope,
            now_ms: 10,
            active_goals: vec![active_task],
            waiting_decision: Some(expired_decision),
            unread_results: vec![result],
        },
    );
    let ledger_index = snapshot
        .sections
        .iter()
        .position(|section| section.kind() == AgentContextSectionKind::LedgerContext)
        .unwrap();
    let memory_snapshot = AgentContextRuntime.inject_memory_recall(
        snapshot,
        &AgentMemoryRecallInput {
            hits: vec![memory_hit(90, MemorySourceOrigin::DebuggingExperience)],
            code_symbol_hits: Vec::new(),
            audience: private_audience(),
        },
    );
    let memory_index = memory_snapshot
        .sections
        .iter()
        .position(|section| section.kind() == AgentContextSectionKind::MemoryRecall)
        .unwrap();
    assert!(ledger_index < memory_index);
    let ledger = &memory_snapshot.sections[ledger_index];
    assert!(ledger.content().contains("active-goal"));
    assert!(ledger.content().contains("goal=完成发布"));
    assert!(ledger.content().contains("plan:\n- 验证构建"));
    assert!(ledger.content().contains("unread-result"));
    assert!(!ledger.content().contains("waiting-decision"));
}

fn private_audience() -> AudienceContext {
    AudienceContext::new(AudienceKind::Private, DisclosurePolicy::PrivateOnly)
}

fn memory_hit(id: u64, origin: MemorySourceOrigin) -> MemoryHybridHit {
    MemoryHybridHit {
        record: MemorySqliteRecord {
            memory_kind: MemoryKind::Knowledge,
            record: MemoryRecord {
                id: MemoryId(id),
                candidate_id: MemoryCandidateId(id),
                state: MemoryState::Strong,
                content_ref: Some(format!("memory-ref:layer-{id}")),
                source_trace: Some(MemorySourceTrace {
                    origin,
                    session_id: Some(SessionId(1)),
                    cell_id: Some(CellId(id)),
                    summary: format!("sha256:layer-{id}"),
                    evidence_ref: Some(format!("timeline://layer/{id}")),
                }),
                tombstone: None,
            },
            namespace_ref: Some("memory-scope:test".to_owned()),
            repo_ref: Some("memory-repo:test".to_owned()),
            user_ref: None,
            created_at_ms: 1_000,
            updated_at_ms: 1_000,
            last_active_at_ms: 1_000,
            ttl_expires_at_ms: None,
            confidence_basis_points: 9_000,
            importance_basis_points: 8_000,
            vector_id: None,
            archived_at_ms: None,
            prompt_text_persisted: false,
            raw_payload_persisted: false,
            remote_write_allowed: false,
        },
        score_basis_points: 8_500,
        text_score: Some(1.0),
        vector_distance: None,
        text_rank: Some(1),
        vector_rank: None,
        explain: MemoryHybridExplain {
            text_component_basis_points: 4_500,
            vector_component_basis_points: 0,
            governance_component_basis_points: 1_000,
            weights: "test",
        },
    }
}
