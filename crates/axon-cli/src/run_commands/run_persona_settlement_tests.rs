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
        signals: None,
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
    .expect("写入 owner 身份");
    (root, config_path)
}

/// 带客观信号的结算输入：验证规则层补位用。
fn settled_input_with_signals<'a>(
    config_path: &'a std::path::Path,
    status: TurnFinalStatus,
    signals: &'a axon_agent_runtime::ExplicitSignals,
) -> PersonaSettlementInput<'a> {
    PersonaSettlementInput {
        config_path,
        session_id: "s-signals",
        assistant_part_id: "part-signals",
        disclosure: DisclosurePolicy::PrivateOnly,
        final_status: status,
        user_affect: None,
        user_fatigue: None,
        persona_drift_bp: None,
        drift_reason: None,
        scene_audience: None,
        signals: Some(signals),
    }
}

/// 深夜且近 30 分钟密集：规则层应给出最重档。
fn dense_late_night_signals() -> axon_agent_runtime::ExplicitSignals {
    axon_agent_runtime::ExplicitSignals {
        hour_bucket: axon_agent_runtime::HourBucket::LateNight,
        consecutive_turns: 9,
        turns_last_30min: 7,
        input_len_delta: -50,
        affect_sequence: Vec::new(),
        failure_streak: 0,
    }
}

fn load_owner_state(root: &std::path::Path) -> axon_persona::domain::persona_state::PersonaState {
    PersonaStore::open_read_only(&persona_store_path(root))
        .expect("库应已创建")
        .load_state("owner_123e4567-e89b-12d3-a456-426614174000")
        .expect("读取状态")
        .expect("状态应存在")
}

#[test]
fn rule_layer_fills_fatigue_when_assessment_is_absent() {
    let (root, config_path) = settle_fixture();
    let signals = dense_late_night_signals();

    settle_persona_turn(&settled_input_with_signals(
        &config_path,
        TurnFinalStatus::Completed,
        &signals,
    ));

    assert_eq!(
        load_owner_state(root.path()).fatigue_level,
        axon_persona::FatigueLevel::High,
        "密集深夜轮次应由规则层补出高疲劳"
    );
}

#[test]
fn assessment_fatigue_always_wins_over_rule_advice() {
    let (root, config_path) = settle_fixture();
    let signals = dense_late_night_signals();
    let mut input = settled_input_with_signals(&config_path, TurnFinalStatus::Completed, &signals);
    input.user_fatigue = Some("low");

    settle_persona_turn(&input);

    assert_eq!(
        load_owner_state(root.path()).fatigue_level,
        axon_persona::FatigueLevel::Low,
        "模型判定优先，规则层不得覆盖"
    );
}

#[test]
fn rule_layer_observes_failure_streak_for_the_state_machine() {
    let (root, config_path) = settle_fixture();
    let signals = axon_agent_runtime::ExplicitSignals {
        failure_streak: 4,
        ..axon_agent_runtime::ExplicitSignals::default()
    };

    settle_persona_turn(&settled_input_with_signals(
        &config_path,
        TurnFinalStatus::Failed,
        &signals,
    ));

    let state = load_owner_state(root.path());
    assert_eq!(state.failure_streak, 4, "观测到的连败必须对齐进状态");
    assert_eq!(state.last_cause, "连续第 4 次失败");
}

#[test]
fn zero_signals_stay_equivalent_to_the_legacy_path() {
    // 攒批开关只影响反思何时跑，不该影响人格状态；「全零信号」必须与
    // 关闭规则层（旧路径）逐字段等价，否则关掉开关就是一次行为回归。
    let (legacy_root, legacy_config) = settle_fixture();
    let (signal_root, signal_config) = settle_fixture();
    let zero = axon_agent_runtime::ExplicitSignals::default();
    assert!(
        rule_advice_for(Some(&zero)).is_none(),
        "全零信号等于没采集，必须被闸掉"
    );

    settle_persona_turn(&settled_input(
        &legacy_config,
        TurnFinalStatus::Completed,
        DisclosurePolicy::PrivateOnly,
        None,
    ));
    // 两个构造器的默认 affect 不同：等价性比较必须先把输入对齐，
    // 否则测到的是构造器差异而不是规则层差异。
    let mut with_signals =
        settled_input_with_signals(&signal_config, TurnFinalStatus::Completed, &zero);
    with_signals.user_affect = Some("positive");
    settle_persona_turn(&with_signals);

    let legacy = load_owner_state(legacy_root.path());
    let with_signals = load_owner_state(signal_root.path());
    assert_eq!(legacy.fatigue_level, with_signals.fatigue_level);
    assert_eq!(legacy.warmth_offset, with_signals.warmth_offset);
    assert_eq!(legacy.failure_streak, with_signals.failure_streak);
    assert_eq!(legacy.last_cause, with_signals.last_cause);
}

#[test]
fn rule_cause_becomes_the_settlement_cause_when_assessment_is_silent() {
    let (root, config_path) = settle_fixture();
    let signals = axon_agent_runtime::ExplicitSignals {
        hour_bucket: axon_agent_runtime::HourBucket::LateNight,
        consecutive_turns: 5,
        turns_last_30min: 7,
        input_len_delta: 0,
        affect_sequence: Vec::new(),
        failure_streak: 0,
    };

    settle_persona_turn(&settled_input_with_signals(
        &config_path,
        TurnFinalStatus::Completed,
        &signals,
    ));

    let state = load_owner_state(root.path());
    assert!(
        state.last_cause.contains("深夜时段"),
        "规则层原因必须落到 last_cause，实际为：{}",
        state.last_cause
    );
}

#[test]
fn assessment_cause_wins_over_rule_cause() {
    let (root, config_path) = settle_fixture();
    let signals = axon_agent_runtime::ExplicitSignals {
        hour_bucket: axon_agent_runtime::HourBucket::LateNight,
        consecutive_turns: 5,
        turns_last_30min: 7,
        input_len_delta: 0,
        affect_sequence: Vec::new(),
        failure_streak: 0,
    };
    let mut input = settled_input_with_signals(&config_path, TurnFinalStatus::Completed, &signals);
    input.user_affect = Some("positive");

    settle_persona_turn(&input);

    assert_eq!(
        load_owner_state(root.path()).last_cause,
        "用户反馈积极",
        "评估器给了判定时，规则层原因不得覆盖"
    );
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
