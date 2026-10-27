//! 人格状态机。
//!
//! 每次偏离基线都必须带可审计的 cause；群聊场景只读不写；子代理不产生事件。

use crate::{
    constants::{PERSONA_DRIFT_GUARD_BP, WARMTH_DELTA_FRUSTRATED, WARMTH_DELTA_POSITIVE},
    domain::{
        persona_event::{PersonaEvent, TurnFinalStatus},
        persona_state::{PersonaState, UserAffect},
        scene_frame::Audience,
    },
};

/// 状态机错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersonaError {
    /// cause 为空。
    EmptyCause,
}

impl std::fmt::Display for PersonaError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyCause => write!(formatter, "人格状态变化必须有非空 cause"),
        }
    }
}

impl std::error::Error for PersonaError {}

/// 一次转移的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    /// 转移后的状态。
    pub state: PersonaState,
    /// 本次变化的原因。
    pub cause: String,
    /// 状态是否有实质变化；`false` 表示调用方不应持久化。
    pub changed: bool,
}

/// 群聊场景的固定说明。
const GROUP_READ_ONLY_CAUSE: &str = "群聊场景（只读）";

/// 执行一次状态转移。
///
/// # Errors
///
/// `Reset` 的 reason 为空时返回 [`PersonaError::EmptyCause`]。
pub fn transition(
    state: &PersonaState,
    event: &PersonaEvent,
    audience: Audience,
) -> Result<Transition, PersonaError> {
    match event {
        PersonaEvent::Reset { reason, now_ms } => {
            let cause = reason.trim();
            if cause.is_empty() {
                return Err(PersonaError::EmptyCause);
            }
            let mut next = PersonaState::baseline(*now_ms);
            next.revision = state.revision.saturating_add(1);
            next.last_cause = cause.to_owned();
            Ok(Transition {
                state: next,
                cause: cause.to_owned(),
                changed: true,
            })
        }
        PersonaEvent::TurnSettled(input) => {
            if audience.is_group() {
                return Ok(Transition {
                    state: state.clone(),
                    cause: GROUP_READ_ONLY_CAUSE.to_owned(),
                    changed: false,
                });
            }
            let (next, cause) = settle(state, input.final_status, input);
            Ok(Transition {
                state: next,
                cause,
                changed: true,
            })
        }
    }
}

fn settle(
    state: &PersonaState,
    final_status: TurnFinalStatus,
    input: &crate::domain::persona_event::TurnSettledInput,
) -> (PersonaState, String) {
    let failure_streak = if final_status.is_failure() {
        state.failure_streak.saturating_add(1)
    } else {
        0
    };
    let familiarity = if final_status == TurnFinalStatus::Completed {
        state.familiarity_saturated()
    } else {
        state.familiarity
    };
    let decayed = i16::from(state.decayed_warmth_offset(input.now_ms));
    let delta = match input.user_affect {
        UserAffect::Positive => i16::from(WARMTH_DELTA_POSITIVE),
        UserAffect::Frustrated => i16::from(WARMTH_DELTA_FRUSTRATED),
        UserAffect::Neutral | UserAffect::Urgent | UserAffect::Unknown => 0,
    };
    let warmth_offset = PersonaState::clamped_offset(decayed + delta);
    let drift_guard = guard_for(input);
    let cause = cause_for(
        final_status,
        input.user_affect,
        failure_streak,
        drift_guard.as_deref(),
    );

    let next = PersonaState {
        revision: state.revision.saturating_add(1),
        warmth_offset,
        familiarity,
        user_affect: input.user_affect,
        failure_streak,
        drift_guard,
        last_cause: cause.clone(),
        updated_at_ms: input.now_ms,
    };
    (next, cause)
}

fn guard_for(input: &crate::domain::persona_event::TurnSettledInput) -> Option<String> {
    if input.persona_drift_bp < PERSONA_DRIFT_GUARD_BP {
        return None;
    }
    let reason = input
        .drift_reason
        .as_deref()
        .map(str::trim)
        .filter(|reason| !reason.is_empty())?;
    Some(reason.to_owned())
}

fn cause_for(
    final_status: TurnFinalStatus,
    affect: UserAffect,
    failure_streak: u8,
    drift_guard: Option<&str>,
) -> String {
    if final_status == TurnFinalStatus::Interrupted {
        return "本轮被中断".to_owned();
    }
    if final_status == TurnFinalStatus::Failed {
        return format!("连续第 {failure_streak} 次失败");
    }
    if let Some(guard) = drift_guard {
        return format!("上一轮表达漂移：{guard}");
    }
    match affect {
        UserAffect::Positive => "用户反馈积极".to_owned(),
        UserAffect::Frustrated => "用户受挫，先致意再解决".to_owned(),
        UserAffect::Urgent => "用户状态紧急，收敛节奏".to_owned(),
        UserAffect::Neutral | UserAffect::Unknown => "任务完成".to_owned(),
    }
}

#[cfg(test)]
#[path = "state_machine_tests.rs"]
mod tests;
