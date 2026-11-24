//! 人格状态与用户情绪。

use serde::{Deserialize, Serialize};

use crate::constants::{
    FAMILIARITY_MAX, WARMTH_DECAY_MAX_HALVINGS, WARMTH_HALF_LIFE_MS, WARMTH_OFFSET_CLAMP,
};

/// 上一轮推断出的用户状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserAffect {
    /// 中性。
    Neutral,
    /// 积极。
    Positive,
    /// 受挫。
    Frustrated,
    /// 紧急。
    Urgent,
    /// 证据不足，未知。
    Unknown,
}

impl UserAffect {
    /// 返回持久化与遥测使用的规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Neutral => "neutral",
            Self::Positive => "positive",
            Self::Frustrated => "frustrated",
            Self::Urgent => "urgent",
            Self::Unknown => "unknown",
        }
    }

    /// 按规范记号解析；未知记号返回 [`UserAffect::Unknown`]。
    #[must_use]
    pub fn parse(token: &str) -> Self {
        match token.trim() {
            "neutral" => Self::Neutral,
            "positive" => Self::Positive,
            "frustrated" => Self::Frustrated,
            "urgent" => Self::Urgent,
            _ => Self::Unknown,
        }
    }

    /// 是否属于需要收敛幽默与篇幅的负向状态。
    #[must_use]
    pub const fn is_distressed(self) -> bool {
        matches!(self, Self::Frustrated | Self::Urgent)
    }
}

/// 按 owner 持久化的人格状态。
///
/// 状态只描述「相对基线的偏移」，身份、底色与表达基线永远由人格内核提供。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonaState {
    /// 乐观并发版本，每次成功结算加一。
    pub revision: u64,
    /// 温度偏移，取值被夹在 [`crate::constants::WARMTH_OFFSET_CLAMP`] 之内。
    pub warmth_offset: i8,
    /// 熟悉度，私聊每完成一轮加一。
    pub familiarity: u8,
    /// 上一轮推断出的用户状态。
    pub user_affect: UserAffect,
    /// 上一轮推断出的用户疲劳度。
    pub fatigue_level: FatigueLevel,
    /// 连续失败或中断的轮数。
    pub failure_streak: u8,
    /// 上一轮表达漂移的纠偏提示，下一轮消费后清空。
    pub drift_guard: Option<String>,
    /// 最近一次变化的原因，必填。
    pub last_cause: String,
    /// 最近一次结算时间，Unix 毫秒。
    pub updated_at_ms: u64,
}

/// 时长为零时的初始原因。
const BASELINE_CAUSE: &str = "初始基线";

impl PersonaState {
    /// 构造基线状态。
    #[must_use]
    pub fn baseline(now_ms: u64) -> Self {
        Self {
            revision: 0,
            warmth_offset: 0,
            familiarity: 0,
            user_affect: UserAffect::Unknown,
            fatigue_level: FatigueLevel::Normal,
            failure_streak: 0,
            drift_guard: None,
            last_cause: BASELINE_CAUSE.to_owned(),
            updated_at_ms: now_ms,
        }
    }

    /// 按半衰期衰减后的温度偏移。
    ///
    /// 衰减只做向下取整的折半，不修改持久化状态；真正的写回发生在结算时。
    #[must_use]
    pub fn decayed_warmth_offset(&self, now_ms: u64) -> i8 {
        if self.warmth_offset == 0 {
            return 0;
        }
        let elapsed = now_ms.saturating_sub(self.updated_at_ms);
        let halvings = elapsed / WARMTH_HALF_LIFE_MS;
        if halvings == 0 {
            return self.warmth_offset;
        }
        let mut value = i16::from(self.warmth_offset);
        for _ in 0..halvings.min(u64::from(WARMTH_DECAY_MAX_HALVINGS)) {
            value /= 2;
        }
        let clamped = value.clamp(
            -i16::from(WARMTH_OFFSET_CLAMP),
            i16::from(WARMTH_OFFSET_CLAMP),
        );
        i8::try_from(clamped).unwrap_or(0)
    }

    /// 把温度偏移夹到允许范围内。
    #[must_use]
    pub fn clamped_offset(value: i16) -> i8 {
        let upper = i16::from(WARMTH_OFFSET_CLAMP);
        i8::try_from(value.clamp(-upper, upper)).unwrap_or(0)
    }

    /// 熟悉度自增并按上限收敛。
    #[must_use]
    pub fn familiarity_saturated(&self) -> u8 {
        self.familiarity.saturating_add(1).min(FAMILIARITY_MAX)
    }

    /// 是否存在待消费的纠偏提示。
    #[must_use]
    pub fn has_drift_guard(&self) -> bool {
        self.drift_guard.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HALF_LIFE: u64 = WARMTH_HALF_LIFE_MS;

    fn state_at(offset: i8, updated_at_ms: u64) -> PersonaState {
        PersonaState {
            warmth_offset: offset,
            updated_at_ms,
            ..PersonaState::baseline(updated_at_ms)
        }
    }

    #[test]
    fn baseline_state_is_neutral_and_empty() {
        let state = PersonaState::baseline(1_000);

        assert_eq!(state.revision, 0);
        assert_eq!(state.warmth_offset, 0);
        assert_eq!(state.familiarity, 0);
        assert_eq!(state.user_affect, UserAffect::Unknown);
        assert_eq!(state.failure_streak, 0);
        assert!(!state.has_drift_guard());
        assert_eq!(state.last_cause, BASELINE_CAUSE);
    }

    #[test]
    fn decay_halves_once_per_half_life() {
        let state = state_at(8, 1_000);

        assert_eq!(state.decayed_warmth_offset(1_000 + HALF_LIFE - 1), 8);
        assert_eq!(state.decayed_warmth_offset(1_000 + HALF_LIFE), 4);
        assert_eq!(state.decayed_warmth_offset(1_000 + HALF_LIFE * 3), 1);
        assert_eq!(state.decayed_warmth_offset(1_000 + HALF_LIFE * 40), 0);
    }

    #[test]
    fn decay_of_negative_offset_stays_symmetric() {
        let state = state_at(-9, 0);

        assert_eq!(state.decayed_warmth_offset(HALF_LIFE), -4);
        assert_eq!(state.decayed_warmth_offset(HALF_LIFE * 2), -2);
        assert_eq!(state.decayed_warmth_offset(HALF_LIFE * 30), 0);
    }

    #[test]
    fn zero_offset_never_decays_into_noise() {
        let state = state_at(0, 0);

        assert_eq!(state.decayed_warmth_offset(u64::MAX / 2), 0);
    }

    #[test]
    fn offset_and_familiarity_clamp_at_bounds() {
        assert_eq!(PersonaState::clamped_offset(999), WARMTH_OFFSET_CLAMP);
        assert_eq!(PersonaState::clamped_offset(-999), -WARMTH_OFFSET_CLAMP);
        assert_eq!(PersonaState::clamped_offset(7), 7);

        let maxed = PersonaState {
            familiarity: FAMILIARITY_MAX,
            ..PersonaState::baseline(0)
        };
        assert_eq!(maxed.familiarity_saturated(), FAMILIARITY_MAX);
        assert_eq!(PersonaState::baseline(0).familiarity_saturated(), 1);
    }

    #[test]
    fn affect_tokens_round_trip_and_unknown_is_safe_default() {
        for affect in [
            UserAffect::Neutral,
            UserAffect::Positive,
            UserAffect::Frustrated,
            UserAffect::Urgent,
            UserAffect::Unknown,
        ] {
            assert_eq!(UserAffect::parse(affect.as_token()), affect);
        }
        assert_eq!(UserAffect::parse("angry"), UserAffect::Unknown);
        assert!(UserAffect::Frustrated.is_distressed());
        assert!(UserAffect::Urgent.is_distressed());
        assert!(!UserAffect::Positive.is_distressed());
    }
}

/// 用户疲劳度：由轮后评估给出，影响表达收敛强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FatigueLevel {
    /// 精力充沛。
    Low,
    /// 常态。
    Normal,
    /// 明显疲劳。
    High,
}

impl FatigueLevel {
    /// 返回持久化与遥测使用的规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }

    /// 从规范记号解析。
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim() {
            "low" => Some(Self::Low),
            "normal" => Some(Self::Normal),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}
