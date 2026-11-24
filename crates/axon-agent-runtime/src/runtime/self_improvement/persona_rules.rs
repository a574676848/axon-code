//! 人格规则层：把零模型客观信号翻成表达建议。
//!
//! 这一层不收模型输入，也不直接写人格状态。它的定位是「评估器缺席时的降级路径」：
//! 有模型判定时一律以模型为准，规则层只补模型没给的那部分。
//!
//! 只输出一档「精力」与一句可读原因。篇幅与语气不在这里另开维度：人格域的
//! `base_voice` 已经以 `fatigue_level` 为输入做收敛，再给一套平行档位只会出现
//! 两套口径互相打架，且没有消费方。
//!
//! 全文件是纯函数、零 IO、零随机：同一组信号必然得到同一份建议，
//! 回放夹具才能在「攒批关」与「攒批开」两种模式下逐轮比对。

use serde::{Deserialize, Serialize};

use crate::turn_trace::{ExplicitSignals, HourBucket};

/// 规则层的默认状态：默认开启。
///
/// 需要停用时把该常量改为 `false` 并重编，或在调用点显式传入 `false`。
pub const PERSONA_RULES_ENABLED_DEFAULT: bool = true;
/// 近 30 分钟轮次密度达到该值即视为高频连轴。
pub const PERSONA_RULES_DENSE_TURNS: u32 = 6;
/// 近 30 分钟轮次密度达到该值即视为透支，直接进最重档。
pub const PERSONA_RULES_OVERDRAWN_TURNS: u32 = 10;
/// 同一会话连续轮数达到该值即视为长会话。
pub const PERSONA_RULES_LONG_SESSION_TURNS: u32 = 25;
/// 建议保留的用户状态序列上限。
pub const PERSONA_RULES_AFFECT_SEQUENCE_MAX: usize = 8;

/// 精力建议；与人格域的疲劳度同构，但本层不依赖人格域类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FatigueAdvice {
    /// 精力充裕。
    Light,
    /// 常态。
    Normal,
    /// 明显透支。
    Heavy,
}

/// 规则层给出的表达建议。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaRuleAdvice {
    /// 精力档：进 `PersonaState.fatigue_level`，再由表达策略消费。
    pub fatigue: FatigueAdvice,
    /// 可读原因：评估器没给判定时进 `last_cause`，出现在下一轮的人格状态段。
    pub cause: String,
}

/// 由零模型信号推导表达建议。
///
/// 判据全部是阈值比较，不看正文语义：`ExplicitSignals` 里没有正文，
/// 从这里往外推任何情绪结论都是编造。因此本函数只输出「该按什么强度说」，
/// 不输出「用户是什么状态」。
#[must_use]
pub fn advise(signals: &ExplicitSignals) -> PersonaRuleAdvice {
    let dense = signals.turns_last_30min >= PERSONA_RULES_DENSE_TURNS;
    let overdrawn = signals.turns_last_30min >= PERSONA_RULES_OVERDRAWN_TURNS;
    let long_session = signals.consecutive_turns >= PERSONA_RULES_LONG_SESSION_TURNS;
    let late_night = signals.hour_bucket == HourBucket::LateNight;
    let failing = signals.failure_streak > 0;

    let fatigue = if overdrawn || long_session || (dense && late_night) {
        FatigueAdvice::Heavy
    } else if dense || late_night || failing {
        FatigueAdvice::Normal
    } else if signals.turns_last_30min <= 1 && signals.consecutive_turns <= 2 {
        FatigueAdvice::Light
    } else {
        FatigueAdvice::Normal
    };

    PersonaRuleAdvice {
        fatigue,
        cause: cause_text(signals, dense, overdrawn, long_session, late_night, failing),
    }
}

/// 把命中的判据拼成一句可读原因；一个都没命中时给常规节奏。
fn cause_text(
    signals: &ExplicitSignals,
    dense: bool,
    overdrawn: bool,
    long_session: bool,
    late_night: bool,
    failing: bool,
) -> String {
    let mut parts = Vec::new();
    if late_night {
        parts.push("深夜时段".to_owned());
    }
    if overdrawn {
        parts.push(format!(
            "近 30 分钟 {} 轮，已透支",
            signals.turns_last_30min
        ));
    } else if dense {
        parts.push(format!(
            "近 30 分钟 {} 轮，节奏密集",
            signals.turns_last_30min
        ));
    }
    if long_session {
        parts.push(format!("会话已连跑 {} 轮", signals.consecutive_turns));
    }
    if failing {
        parts.push(format!("连续 {} 轮失败", signals.failure_streak));
    }
    if parts.is_empty() {
        return "常规节奏".to_owned();
    }
    parts.join("；")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(
        hour_bucket: HourBucket,
        consecutive_turns: u32,
        turns_last_30min: u32,
        input_len_delta: i32,
        failure_streak: u32,
    ) -> ExplicitSignals {
        ExplicitSignals {
            hour_bucket,
            consecutive_turns,
            turns_last_30min,
            input_len_delta,
            affect_sequence: Vec::new(),
            failure_streak,
        }
    }

    #[test]
    fn rules_default_to_enabled() {
        const { assert!(PERSONA_RULES_ENABLED_DEFAULT, "规则层常量默认必须开启") };
    }

    #[test]
    fn quiet_afternoon_session_stays_light() {
        let advice = advise(&signals(HourBucket::Afternoon, 1, 1, 120, 0));

        assert_eq!(advice.fatigue, FatigueAdvice::Light);
        assert_eq!(advice.cause, "常规节奏");
    }

    #[test]
    fn dense_late_night_goes_heavy() {
        let advice = advise(&signals(HourBucket::LateNight, 8, 7, 0, 0));

        assert_eq!(advice.fatigue, FatigueAdvice::Heavy);
        assert!(advice.cause.contains("深夜时段"));
        assert!(advice.cause.contains("节奏密集"));
    }

    #[test]
    fn single_failure_lifts_to_normal_and_names_the_streak() {
        let advice = advise(&signals(HourBucket::Morning, 6, 2, 0, 2));

        assert_eq!(advice.fatigue, FatigueAdvice::Normal);
        assert!(advice.cause.contains("连续 2 轮失败"));
    }

    #[test]
    fn overdrawn_density_and_long_session_pin_heavy() {
        let overdrawn = advise(&signals(HourBucket::Morning, 4, 12, 0, 0));
        let long = advise(&signals(HourBucket::Evening, 30, 2, 0, 0));

        assert_eq!(overdrawn.fatigue, FatigueAdvice::Heavy);
        assert_eq!(long.fatigue, FatigueAdvice::Heavy);
        assert!(overdrawn.cause.contains("已透支"));
        assert!(long.cause.contains("会话已连跑 30 轮"));
    }

    #[test]
    fn advice_is_deterministic_for_the_same_signals() {
        let input = signals(HourBucket::LateNight, 27, 9, -120, 1);

        assert_eq!(advise(&input), advise(&input));
    }
}
