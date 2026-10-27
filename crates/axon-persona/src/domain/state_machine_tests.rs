//! 状态机边界用例。

use super::*;
use crate::constants::WARMTH_HALF_LIFE_MS;
use crate::domain::persona_event::TurnSettledInput;

fn settled(
    final_status: TurnFinalStatus,
    affect: UserAffect,
    drift_bp: u16,
    drift_reason: Option<&str>,
    now_ms: u64,
) -> PersonaEvent {
    PersonaEvent::TurnSettled(TurnSettledInput {
        final_status,
        user_affect: affect,
        fatigue_level: Some(crate::domain::persona_state::FatigueLevel::Normal),
        persona_drift_bp: drift_bp,
        drift_reason: drift_reason.map(ToOwned::to_owned),
        now_ms,
    })
}

fn completed(affect: UserAffect, now_ms: u64) -> PersonaEvent {
    settled(TurnFinalStatus::Completed, affect, 0, None, now_ms)
}

fn apply(state: &PersonaState, event: &PersonaEvent) -> Transition {
    transition(state, event, Audience::Private).expect("私聊结算不应失败")
}

#[test]
fn completed_turn_resets_streak_and_raises_familiarity() {
    let state = PersonaState {
        failure_streak: 3,
        familiarity: 7,
        ..PersonaState::baseline(0)
    };

    let next = apply(&state, &completed(UserAffect::Neutral, 1_000));

    assert_eq!(next.state.failure_streak, 0);
    assert_eq!(next.state.familiarity, 8);
    assert_eq!(next.cause, "任务完成");
    assert!(next.changed);
}

#[test]
fn failure_increments_streak_without_touching_familiarity() {
    let state = PersonaState {
        failure_streak: 1,
        familiarity: 7,
        ..PersonaState::baseline(0)
    };

    let failed = apply(
        &state,
        &settled(TurnFinalStatus::Failed, UserAffect::Neutral, 0, None, 1_000),
    );
    assert_eq!(failed.state.failure_streak, 2);
    assert_eq!(failed.state.familiarity, 7);
    assert_eq!(failed.cause, "连续第 2 次失败");

    let interrupted = apply(
        &state,
        &settled(
            TurnFinalStatus::Interrupted,
            UserAffect::Neutral,
            0,
            None,
            1_000,
        ),
    );
    assert_eq!(interrupted.state.failure_streak, 2);
    assert_eq!(interrupted.cause, "本轮被中断");
}

#[test]
fn affect_adjusts_warmth_and_is_recorded() {
    let base = PersonaState::baseline(0);

    let positive = apply(&base, &completed(UserAffect::Positive, 100)).state;
    assert_eq!(positive.warmth_offset, 5);
    assert_eq!(positive.user_affect, UserAffect::Positive);

    let frustrated = apply(&base, &completed(UserAffect::Frustrated, 100)).state;
    assert_eq!(frustrated.warmth_offset, 3);
    assert_eq!(frustrated.user_affect, UserAffect::Frustrated);

    let urgent = apply(&base, &completed(UserAffect::Urgent, 100)).state;
    assert_eq!(urgent.warmth_offset, 0);
}

#[test]
fn warmth_offset_is_clamped_and_decays_before_accumulating() {
    let hot = PersonaState {
        warmth_offset: 18,
        ..PersonaState::baseline(0)
    };
    let clamped = apply(&hot, &completed(UserAffect::Positive, 100)).state;
    assert_eq!(clamped.warmth_offset, 20);

    // 一个半衰期后先折半（18 -> 9），再叠加 +5。
    let decayed = apply(&hot, &completed(UserAffect::Positive, WARMTH_HALF_LIFE_MS)).state;
    assert_eq!(decayed.warmth_offset, 14);
}

#[test]
fn repeated_failures_extend_streak_and_keep_cause_auditable() {
    let mut state = PersonaState::baseline(0);
    for round in 1u8..=3 {
        let next = apply(
            &state,
            &settled(
                TurnFinalStatus::Failed,
                UserAffect::Neutral,
                0,
                None,
                u64::from(round) * 10,
            ),
        );
        assert_eq!(next.state.failure_streak, round);
        assert!(next.cause.contains(&round.to_string()));
        state = next.state;
    }
}

#[test]
fn drift_above_threshold_sets_guard_and_explains_cause() {
    let event = settled(
        TurnFinalStatus::Completed,
        UserAffect::Neutral,
        crate::constants::PERSONA_DRIFT_GUARD_BP,
        Some("过度寒暄"),
        1_000,
    );

    let next = apply(&PersonaState::baseline(0), &event);

    assert_eq!(next.state.drift_guard.as_deref(), Some("过度寒暄"));
    assert!(next.cause.contains("过度寒暄"));
}

#[test]
fn drift_below_threshold_or_without_reason_clears_guard() {
    let guarded = PersonaState {
        drift_guard: Some("旧的偏差".to_owned()),
        ..PersonaState::baseline(0)
    };

    let below = apply(
        &guarded,
        &settled(
            TurnFinalStatus::Completed,
            UserAffect::Neutral,
            crate::constants::PERSONA_DRIFT_GUARD_BP - 1,
            Some("微不足道"),
            1_000,
        ),
    );
    assert_eq!(below.state.drift_guard, None);

    let reasonless = apply(
        &guarded,
        &settled(
            TurnFinalStatus::Completed,
            UserAffect::Neutral,
            crate::constants::PERSONA_DRIFT_GUARD_BP,
            None,
            1_000,
        ),
    );
    assert_eq!(reasonless.state.drift_guard, None);
}

#[test]
fn group_audience_is_read_only() {
    let state = PersonaState {
        revision: 9,
        failure_streak: 4,
        ..PersonaState::baseline(0)
    };

    let transition = transition(
        &state,
        &completed(UserAffect::Positive, 1_000),
        Audience::Group,
    )
    .expect("群聊结算不报错");

    assert!(!transition.changed);
    assert_eq!(transition.state, state);
    assert_eq!(transition.cause, "群聊场景（只读）");
}

#[test]
fn empty_reset_reason_is_rejected() {
    let error = transition(
        &PersonaState::baseline(0),
        &PersonaEvent::Reset {
            reason: "   ".to_owned(),
            now_ms: 1,
        },
        Audience::Private,
    );

    assert_eq!(error, Err(PersonaError::EmptyCause));
}

#[test]
fn reset_returns_to_baseline_and_keeps_revision_monotonic() {
    let state = PersonaState {
        revision: 12,
        warmth_offset: 9,
        familiarity: 40,
        failure_streak: 2,
        drift_guard: Some("偏差".to_owned()),
        ..PersonaState::baseline(0)
    };

    let transition = transition(
        &state,
        &PersonaEvent::Reset {
            reason: "用户手动重置".to_owned(),
            now_ms: 5_000,
        },
        Audience::Private,
    )
    .expect("重置应成功");

    assert_eq!(transition.state.revision, 13);
    assert_eq!(transition.state.warmth_offset, 0);
    assert_eq!(transition.state.familiarity, 0);
    assert_eq!(transition.state.failure_streak, 0);
    assert_eq!(transition.state.drift_guard, None);
    assert_eq!(transition.state.last_cause, "用户手动重置");
    assert_eq!(transition.state.updated_at_ms, 5_000);
}

#[test]
fn revision_increases_by_exactly_one_per_settlement() {
    let state = PersonaState::baseline(0);

    let first = apply(&state, &completed(UserAffect::Neutral, 10));
    let second = apply(&first.state, &completed(UserAffect::Neutral, 20));

    assert_eq!(first.state.revision, 1);
    assert_eq!(second.state.revision, 2);
    assert_eq!(second.state.updated_at_ms, 20);
}

#[test]
fn fatigue_decays_toward_normal_then_accepts_a_fresh_signal() {
    use crate::domain::persona_state::FatigueLevel;

    let mut state = PersonaState::baseline(0);
    state.fatigue_level = FatigueLevel::High;

    // 两个半衰期后回落到常态。
    let two_half_lives = crate::constants::FATIGUE_HALF_LIFE_MS * 2;
    let (next, _) = settle(
        &state,
        TurnFinalStatus::Completed,
        &TurnSettledInput {
            final_status: TurnFinalStatus::Completed,
            user_affect: UserAffect::Unknown,
            fatigue_level: None,
            persona_drift_bp: 0,
            drift_reason: None,
            now_ms: two_half_lives,
        },
    );
    assert_eq!(next.fatigue_level, FatigueLevel::Normal, "两个半衰期应回落到常态");

    // 本轮给出明确信号时，覆盖衰减结果。
    let (next, _) = settle(
        &state,
        TurnFinalStatus::Completed,
        &TurnSettledInput {
            final_status: TurnFinalStatus::Completed,
            user_affect: UserAffect::Unknown,
            fatigue_level: Some(FatigueLevel::Low),
            persona_drift_bp: 0,
            drift_reason: None,
            now_ms: 1_000,
        },
    );
    assert_eq!(next.fatigue_level, FatigueLevel::Low, "新信号应覆盖衰减值");
}