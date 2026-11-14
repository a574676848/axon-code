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

    // 地点桶必须是自己造出来的临时仓库，不能借用跑测试这台机器的目录：
    // 开发机上 E:/zbg/axon-code 存在，断言就能过，CI 上同一个字面量指向空路径，
    // 于是同一份代码在三台 runner 上判成 other。
    let workspace = git_workspace_fixture();

    let cards = super::reflection_cards(&trace, Some(&assessment), &workspace);

    assert_eq!(cards.len(), 1, "无回执的轮次也要给一张索引卡");
    assert_eq!(cards[0].trace_id, "trace:conversation-1:0");
    assert_eq!(
        cards[0].signals.specificity,
        axon_agent_runtime::EXECUTION_SPECIFICITY_MIN_BP,
        "v3 输出没有 specificity 字段，必须折中到门槛，不能落 0"
    );
    assert_eq!(cards[0].location.workspace_kind, "project");
}

/// 反思地点分桶的三个类别：只依赖临时目录，不依赖宿主机上恰好存在什么。
#[test]
fn workspace_kind_buckets_home_project_and_other() {
    let root = tempfile::tempdir().expect("临时根目录");

    let home = root.path().join(".axon");
    std::fs::create_dir_all(&home).expect("创建 Home 桶");
    assert_eq!(cards_for(&home)[0].location.workspace_kind, "home");

    let project = root.path().join("project");
    std::fs::create_dir_all(project.join(".git")).expect("创建 .git 目录");
    assert_eq!(cards_for(&project)[0].location.workspace_kind, "project");

    // worktree / submodule 的 `.git` 不是目录，是指向 gitdir 的单行文件。
    let worktree = root.path().join("worktree");
    std::fs::create_dir_all(&worktree).expect("创建 worktree 目录");
    std::fs::write(
        worktree.join(".git"),
        "gitdir: ../elsewhere/.git/worktrees/x",
    )
    .expect("写入 .git 文件");
    assert_eq!(
        cards_for(&worktree)[0].location.workspace_kind,
        "project",
        ".git 是文件时同样必须算项目，否则 worktree 会被并进 other 桶"
    );

    let plain = root.path().join("plain");
    std::fs::create_dir_all(&plain).expect("创建普通目录");
    assert_eq!(cards_for(&plain)[0].location.workspace_kind, "other");
}

/// 造一个带 `.git` 的临时项目目录，路径由临时目录保证在任何 runner 上都存在。
fn git_workspace_fixture() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("axon-salience-{}", std::process::id()));
    std::fs::create_dir_all(root.join(".git")).expect("创建临时项目 .git");
    root
}

fn cards_for(cwd: &std::path::Path) -> Vec<axon_agent_runtime::ReflectionCard> {
    super::reflection_cards(&trace(), Some(&assessment()), cwd)
}

fn trace() -> axon_agent_runtime::TurnTrace {
    axon_agent_runtime::TurnTrace::from_outcome(
        &envelope(),
        "trace:conversation-1:0",
        1_000,
        axon_agent_runtime::ExplicitSignals::default(),
    )
}

fn assessment() -> SemanticSalienceAssessment {
    SemanticSalienceAssessment {
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
    }
}
/// 反思地点分桶夹具共用的空信封。
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
