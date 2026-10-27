//! 人格结算与风格采集的判定边界。

use super::*;

#[test]
fn private_round_may_write_state() {
    assert!(persona_write_allowed(
        DisclosurePolicy::PrivateOnly,
        Some("private")
    ));
    assert!(persona_write_allowed(DisclosurePolicy::PrivateOnly, None));
}

#[test]
fn group_round_is_read_only_by_either_signal() {
    assert!(!persona_write_allowed(
        DisclosurePolicy::PrivateOnly,
        Some("group")
    ));
    assert!(!persona_write_allowed(
        DisclosurePolicy::Workspace,
        Some("private")
    ));
    assert!(!persona_write_allowed(DisclosurePolicy::Global, None));
}

#[test]
fn style_evidence_must_cite_the_user_message() {
    let refs = vec![
        "tool:call-1".to_owned(),
        "user:turn-7".to_owned(),
        "memory:3".to_owned(),
    ];

    assert_eq!(
        select_user_evidence(&refs, Some("user:turn-7")).map(String::as_str),
        Some("user:turn-7")
    );
    assert_eq!(select_user_evidence(&refs, Some("user:turn-8")), None);
    assert_eq!(select_user_evidence(&refs, None), None);
}

#[test]
fn empty_evidence_never_selects_anything() {
    let refs: Vec<String> = Vec::new();

    assert_eq!(select_user_evidence(&refs, Some("user:turn-7")), None);
}

#[test]
fn settlement_idempotency_key_is_stable_per_turn_and_status() {
    let completed = settlement_idempotency_key("s-1", "part-1", TurnFinalStatus::Completed);
    let same_turn = settlement_idempotency_key("s-1", "part-1", TurnFinalStatus::Completed);
    let other_status = settlement_idempotency_key("s-1", "part-1", TurnFinalStatus::Failed);
    let other_turn = settlement_idempotency_key("s-1", "part-2", TurnFinalStatus::Completed);

    assert_eq!(completed, same_turn, "同轮同终态必须同键");
    assert_ne!(completed, other_status, "不同终态必须是不同键");
    assert_ne!(completed, other_turn, "不同轮次必须是不同键");
}

fn settle_fixture() -> (axon_test_support::TestTempDir, std::path::PathBuf) {
    let root = axon_test_support::test_temp_dir();
    let config_path = root.path().join("config.toml");
    std::fs::write(&config_path, "[agent]\n").expect("写入配置");
    let identity = root.path().join("identity");
    std::fs::create_dir_all(&identity).expect("创建身份目录");
    std::fs::write(
        identity.join("owner.json"),
        r#"{"schemaVersion":1,"ownerId":"owner_123e4567-e89b-12d3-a456-426614174000"}"#,
    )
    .expect("写入 owner 身份");
    (root, config_path)
}

fn settled_input<'a>(
    config_path: &'a std::path::Path,
    status: TurnFinalStatus,
    disclosure: DisclosurePolicy,
    scene: Option<&'a str>,
) -> PersonaSettlementInput<'a> {
    PersonaSettlementInput {
        config_path,
        session_id: "s-1",
        assistant_part_id: "part-1",
        disclosure,
        final_status: status,
        user_affect: Some("positive"),
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        scene_audience: scene,
    }
}

#[test]
fn private_completion_writes_state_and_replays_idempotently() {
    let (root, config_path) = settle_fixture();
    let input = settled_input(
        &config_path,
        TurnFinalStatus::Completed,
        DisclosurePolicy::PrivateOnly,
        None,
    );

    settle_persona_turn(&input);
    settle_persona_turn(&input);

    let store = PersonaStore::open_read_only(&persona_store_path(root.path())).expect("库应已创建");
    let owner = "owner_123e4567-e89b-12d3-a456-426614174000";
    let state = store
        .load_state(owner)
        .expect("读取状态")
        .expect("状态应存在");
    assert_eq!(state.revision, 1, "同一轮重放不得重复结算");
    assert_eq!(state.user_affect.as_token(), "positive");
    assert_eq!(state.familiarity, 1);
}

#[test]
fn group_round_never_touches_the_store() {
    let (root, config_path) = settle_fixture();

    settle_persona_turn(&settled_input(
        &config_path,
        TurnFinalStatus::Completed,
        DisclosurePolicy::Workspace,
        None,
    ));
    settle_persona_turn(&settled_input(
        &config_path,
        TurnFinalStatus::Completed,
        DisclosurePolicy::PrivateOnly,
        Some("group"),
    ));

    assert!(
        !persona_store_path(root.path()).exists(),
        "群聊轮不得创建或写入人格库"
    );
}

#[test]
fn failed_round_accumulates_failure_streak() {
    let (root, config_path) = settle_fixture();

    settle_persona_turn(&settled_input(
        &config_path,
        TurnFinalStatus::Failed,
        DisclosurePolicy::PrivateOnly,
        None,
    ));

    let store = PersonaStore::open_read_only(&persona_store_path(root.path())).expect("库应已创建");
    let state = store
        .load_state("owner_123e4567-e89b-12d3-a456-426614174000")
        .expect("读取状态")
        .expect("状态应存在");
    assert_eq!(state.failure_streak, 1);
    assert_eq!(state.last_cause, "连续第 1 次失败");
}

fn assessment_with_style(evidence_ref: &str, text: &str) -> anti_assessment::Assessment {
    anti_assessment::Assessment::style(evidence_ref, text)
}

mod anti_assessment {
    use axon_agent_runtime::{
        SEMANTIC_SALIENCE_SCHEMA_VERSION, SemanticMemoryCandidateType, SemanticSalienceAssessment,
        SemanticSalienceSignals,
    };

    pub(super) struct Assessment(pub(super) SemanticSalienceAssessment);

    impl Assessment {
        pub(super) fn style(evidence_ref: &str, text: &str) -> Self {
            Self(SemanticSalienceAssessment {
                schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
                should_reflect: true,
                candidate_types: vec![SemanticMemoryCandidateType::Style],
                signals: SemanticSalienceSignals::default(),
                evidence_refs: vec![evidence_ref.to_owned()],
                reason: "用户明确要求少寒暄".to_owned(),
                user_affect: Some("positive".to_owned()),
                user_fatigue: None,
                persona_drift_bp: Some(0),
                drift_reason: None,
                style_candidate: Some(text.to_owned()),
            })
        }
    }
}

fn style_fixture() -> (axon_test_support::TestTempDir, std::path::PathBuf) {
    let (root, config_path) = settle_fixture();
    std::fs::write(
        root.path().join("SOUL.md"),
        "---\nschema: axon-soul/v1\nidentity:\n  name: Axon\n  address: 长官\n  role: 副官\nbackstory: 来处。\ninvariants:\n  - 依据事实作答\ntraits: [沉稳]\nvoice:\n  warmth: 40\n  formality: 60\n  humor: subtle\n  verbosity: concise\n---\n",
    )
    .expect("写入人设");
    (root, config_path)
}

#[test]
fn style_candidate_is_recorded_then_published_into_stable_segment() {
    let (root, config_path) = style_fixture();
    let assessment = assessment_with_style("user:turn-7", "少用寒暄，结论先行");

    capture_style_candidate(
        &config_path,
        DisclosurePolicy::PrivateOnly,
        Some(&assessment.0),
        Some("user:turn-7"),
    );

    let path = persona_store_path(root.path());
    let mut store = PersonaStore::open(&path).expect("库应已创建");
    let owner = "owner_123e4567-e89b-12d3-a456-426614174000";
    let pending = store.list_styles(owner, true).expect("列举风格");
    assert_eq!(pending.len(), 1, "候选应入 pending");
    assert_eq!(pending[0].status, "pending");
    assert_eq!(pending[0].text, "少用寒暄，结论先行");

    let revision = store
        .publish_style_if_due(owner, axon_time::unix_now_ms() + 1)
        .expect("发布");
    assert_eq!(revision, Some(1));
    let published = store.published_style(owner).expect("读取已发布");
    assert_eq!(published.entries.len(), 1);
    assert_eq!(published.entries[0].text, "少用寒暄，结论先行");
}

#[test]
fn style_candidate_is_dropped_when_evidence_misses_the_user_message() {
    let (root, config_path) = style_fixture();
    let assessment = assessment_with_style("tool:call-1", "少用寒暄");

    capture_style_candidate(
        &config_path,
        DisclosurePolicy::PrivateOnly,
        Some(&assessment.0),
        Some("user:turn-7"),
    );

    assert!(
        !persona_store_path(root.path()).exists(),
        "证据未命中用户消息时不得落库"
    );
}

#[test]
fn style_candidate_is_dropped_for_group_rounds() {
    let (root, config_path) = style_fixture();
    let assessment = assessment_with_style("user:turn-7", "少用寒暄");

    capture_style_candidate(
        &config_path,
        DisclosurePolicy::Workspace,
        Some(&assessment.0),
        Some("user:turn-7"),
    );

    assert!(
        !persona_store_path(root.path()).exists(),
        "群聊轮不得采集风格"
    );
}
