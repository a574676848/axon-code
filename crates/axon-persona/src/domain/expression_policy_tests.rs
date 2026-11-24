//! 表达策略边界用例。

use super::*;
use crate::constants::{PERSONA_DRIFT_GUARD_BP, REUNION_IDLE_MS};
use crate::domain::{
    persona_core::{Identity, RuleCondition, SceneRule},
    persona_state::UserAffect,
    scene_frame::Channel,
    voice::{HumorLevel, Verbosity, VoicePatch},
};

fn core_with(playful: bool, rules: Vec<SceneRule>) -> PersonaCore {
    PersonaCore {
        identity: Identity {
            name: "Axon".to_owned(),
            address: "长官".to_owned(),
            role: "战术副官".to_owned(),
        },
        backstory: "来自工程一线。".to_owned(),
        invariants: vec!["依据事实作答".to_owned()],
        traits: vec!["沉稳".to_owned()],
        voice: Voice::new(
            40,
            60,
            if playful {
                HumorLevel::Playful
            } else {
                HumorLevel::Subtle
            },
            Verbosity::Detailed,
        ),
        rules,
        notes: None,
    }
}

fn scene(audience: Audience, has_workspace: bool, daypart: Daypart) -> SceneFrame {
    SceneFrame::new(Channel::Cli, audience, daypart, has_workspace, 0)
}

#[test]
fn focus_downgrades_humor_one_level() {
    let directive = resolve_expression(
        &core_with(true, Vec::new()),
        &PersonaState::baseline(0),
        &scene(Audience::Private, true, Daypart::Afternoon),
        0,
    );

    assert_eq!(directive.mode, PersonaMode::Focus);
    assert_eq!(directive.voice.humor, HumorLevel::Subtle);
}

#[test]
fn incident_forces_minimal_verbosity_and_no_humor() {
    let state = PersonaState {
        user_affect: UserAffect::Frustrated,
        fatigue_level: crate::domain::persona_state::FatigueLevel::Normal,
        ..PersonaState::baseline(0)
    };

    let directive = resolve_expression(
        &core_with(true, Vec::new()),
        &state,
        &scene(Audience::Private, true, Daypart::Evening),
        0,
    );

    assert_eq!(directive.mode, PersonaMode::Incident);
    assert_eq!(directive.voice.humor, HumorLevel::Off);
    assert_eq!(directive.voice.verbosity, Verbosity::Minimal);
}

#[test]
fn group_hides_private_inference_and_raises_formality() {
    let state = PersonaState {
        user_affect: UserAffect::Frustrated,
        fatigue_level: crate::domain::persona_state::FatigueLevel::Normal,
        last_cause: "用户受挫，先致意再解决".to_owned(),
        drift_guard: Some("上一轮过度寒暄".to_owned()),
        ..PersonaState::baseline(0)
    };

    let directive = resolve_expression(
        &core_with(true, Vec::new()),
        &state,
        &scene(Audience::Group, true, Daypart::Afternoon),
        0,
    );

    assert_eq!(directive.mode, PersonaMode::Group);
    assert_eq!(directive.voice.humor, HumorLevel::Off);
    assert_eq!(directive.voice.formality, 80);
    assert_eq!(directive.cause, "群聊场景");
    assert_eq!(directive.guard.as_deref(), Some("回归基线"));
    assert!(!directive.cause.contains("受挫"));
}

#[test]
fn first_matching_rule_wins_over_mode_defaults() {
    let rules = vec![
        SceneRule {
            when: RuleCondition {
                mode: Some(PersonaMode::Focus),
                audience: None,
                user_affect: None,
                domain: None,
                fatigue_level: None,
            },
            voice: VoicePatch {
                verbosity: Some(Verbosity::Minimal),
                ..VoicePatch::default()
            },
        },
        SceneRule {
            when: RuleCondition::default(),
            voice: VoicePatch::default(),
        },
    ];

    let directive = resolve_expression(
        &core_with(false, rules),
        &PersonaState::baseline(0),
        &scene(Audience::Private, true, Daypart::Afternoon),
        0,
    );

    assert_eq!(directive.voice.verbosity, Verbosity::Minimal);
}

#[test]
fn warmth_offset_applies_only_in_casual_mode() {
    let state = PersonaState {
        warmth_offset: 10,
        ..PersonaState::baseline(0)
    };

    let casual = resolve_expression(
        &core_with(false, Vec::new()),
        &state,
        &scene(Audience::Private, false, Daypart::Afternoon),
        0,
    );
    let focus = resolve_expression(
        &core_with(false, Vec::new()),
        &state,
        &scene(Audience::Private, true, Daypart::Afternoon),
        0,
    );

    assert_eq!(casual.mode, PersonaMode::Casual);
    assert_eq!(casual.voice.warmth, 50);
    assert_eq!(focus.voice.warmth, 40);
}

#[test]
fn late_night_caps_verbosity_but_keeps_other_fields() {
    let directive = resolve_expression(
        &core_with(false, Vec::new()),
        &PersonaState::baseline(0),
        &scene(Audience::Private, false, Daypart::LateNight),
        0,
    );

    assert_eq!(directive.voice.verbosity, Verbosity::Concise);
    assert_eq!(directive.voice.warmth, 40);
}

#[test]
fn reunion_greeting_needs_long_idle_and_familiarity() {
    let core = core_with(false, Vec::new());
    let familiar = PersonaState {
        familiarity: 20,
        ..PersonaState::baseline(0)
    };
    let stranger = PersonaState::baseline(0);
    let long_idle = SceneFrame::new(
        Channel::Cli,
        Audience::Private,
        Daypart::Evening,
        false,
        REUNION_IDLE_MS,
    );
    let short_idle = SceneFrame::new(
        Channel::Cli,
        Audience::Private,
        Daypart::Evening,
        false,
        REUNION_IDLE_MS - 1,
    );

    assert!(resolve_expression(&core, &familiar, &long_idle, 0).allow_reunion_greeting);
    assert!(!resolve_expression(&core, &familiar, &short_idle, 0).allow_reunion_greeting);
    assert!(!resolve_expression(&core, &stranger, &long_idle, 0).allow_reunion_greeting);
}

#[test]
fn drift_guard_passes_through_for_private_but_not_group() {
    let state = PersonaState {
        drift_guard: Some("过度寒暄".to_owned()),
        ..PersonaState::baseline(0)
    };
    let core = core_with(false, Vec::new());

    let private = resolve_expression(
        &core,
        &state,
        &scene(Audience::Private, false, Daypart::Afternoon),
        0,
    );
    assert_eq!(private.guard.as_deref(), Some("过度寒暄"));

    let group = resolve_expression(
        &core,
        &state,
        &scene(Audience::Group, false, Daypart::Afternoon),
        0,
    );
    assert_eq!(group.guard.as_deref(), Some("回归基线"));
}

#[test]
fn drift_threshold_constant_matches_contract() {
    assert_eq!(PERSONA_DRIFT_GUARD_BP, 7_000);
    assert!(hides_affect(Audience::Group));
    assert!(!hides_affect(Audience::Private));
}
