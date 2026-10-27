//! 表达策略：把内核、状态与场景合成为本轮表达指令。
//!
//! 求值顺序固定为：表达基线、模式默认、命中规则、状态偏移、安全收敛。
//! 不可违背项不参与求值，它们由稳定段渲染始终置于首位。

use crate::{
    constants::{GROUP_FORMALITY_BONUS, REUNION_IDLE_MS, REUNION_MIN_FAMILIARITY},
    domain::{
        persona_core::{PersonaCore, RuleContext},
        persona_state::PersonaState,
        scene_frame::{Audience, Daypart, PersonaMode, SceneFrame, resolve_mode},
        voice::{HumorLevel, Verbosity, Voice},
    },
};

/// 群聊场景的固定原因说明，避免泄露私聊推断。
const GROUP_CAUSE: &str = "群聊场景";
/// 群聊场景下纠偏提示的脱敏表述。
const GROUP_GUARD: &str = "回归基线";

/// 本轮表达指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpressionDirective {
    /// 推导出的模式。
    pub mode: PersonaMode,
    /// 最终表达参数。
    pub voice: Voice,
    /// 本轮变化原因；群聊场景固定为脱敏表述。
    pub cause: String,
    /// 待纠偏提示；群聊场景不带原文。
    pub guard: Option<String>,
    /// 是否允许一句简短的久别致意。
    pub allow_reunion_greeting: bool,
}

/// 按内核、状态与场景求解本轮表达指令。
#[must_use]
pub fn resolve_expression(
    core: &PersonaCore,
    state: &PersonaState,
    scene: &SceneFrame,
    now_ms: u64,
) -> ExpressionDirective {
    let mode = resolve_mode(state, scene);
    let context = RuleContext {
        mode,
        audience: scene.audience,
        user_affect: state.user_affect,
        domain: scene.domain,
        fatigue_level: state.fatigue_level,
    };
    let voice = base_voice(core, mode, context);
    let voice = apply_state_offset(voice, state, mode, now_ms);
    let voice = converge(voice, scene.daypart);
    let group = scene.audience.is_group();

    ExpressionDirective {
        mode,
        voice,
        cause: if group {
            GROUP_CAUSE.to_owned()
        } else {
            state.last_cause.clone()
        },
        guard: state.drift_guard.as_ref().map(|_| {
            if group {
                GROUP_GUARD.to_owned()
            } else {
                state.drift_guard.clone().unwrap_or_default()
            }
        }),
        allow_reunion_greeting: allow_reunion(state, scene),
    }
}

fn base_voice(core: &PersonaCore, mode: PersonaMode, context: RuleContext) -> Voice {
    let base = match mode {
        PersonaMode::Focus => Voice {
            humor: core.voice.humor.downgrade(),
            ..core.voice
        },
        PersonaMode::Incident => Voice {
            humor: HumorLevel::Off,
            verbosity: Verbosity::Minimal,
            ..core.voice
        },
        PersonaMode::Group => Voice {
            humor: HumorLevel::Off,
            ..core.voice.with_formality_delta(GROUP_FORMALITY_BONUS as i8)
        },
        PersonaMode::Casual => core.voice,
    };
    match core.first_matching_rule(context) {
        Some(rule) => base.apply(rule.voice),
        None => base,
    }
}

fn apply_state_offset(voice: Voice, state: &PersonaState, mode: PersonaMode, now_ms: u64) -> Voice {
    if mode != PersonaMode::Casual {
        return voice;
    }
    voice.with_warmth_delta(state.decayed_warmth_offset(now_ms))
}

fn converge(voice: Voice, daypart: Daypart) -> Voice {
    if daypart != Daypart::LateNight {
        return voice;
    }
    Voice {
        verbosity: voice.verbosity.cap(Verbosity::Concise),
        ..voice
    }
}

fn allow_reunion(state: &PersonaState, scene: &SceneFrame) -> bool {
    !scene.audience.is_group()
        && scene.idle_gap_ms >= REUNION_IDLE_MS
        && state.familiarity >= REUNION_MIN_FAMILIARITY
}

/// 判断给定受众下是否需要隐藏情绪推断。
#[must_use]
pub const fn hides_affect(audience: Audience) -> bool {
    audience.is_group()
}

#[cfg(test)]
#[path = "expression_policy_tests.rs"]
mod tests;
