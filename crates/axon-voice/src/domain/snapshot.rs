//! 人格只读快照：语音侧唯一的派生输入。
//!
//! 适配层把 `/api/v1/persona/summary` 映射成本结构；`directive` 是人格系统已合成的
//! 表达指令（含模式、规则、状态偏移与时段收敛），`state` 只保留语音侧仍需要的少量字段。
//! 语音侧不得再自行叠加 `warmth_offset` 或 `daypart`。

use crate::domain::profile::Verbosity;

/// 人格模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PersonaMode {
    /// 专注。
    Focus,
    /// 事故处理。
    Incident,
    /// 群聊。
    Group,
    /// 日常。
    Casual,
}

impl PersonaMode {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Incident => "incident",
            Self::Group => "group",
            Self::Casual => "casual",
        }
    }
}

/// 幽默档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HumorLevel {
    /// 关闭。
    Off,
    /// 含蓄。
    Subtle,
    /// 活泼。
    Playful,
}

/// 用户情绪。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserAffect {
    /// 中性。
    Neutral,
    /// 积极。
    Positive,
    /// 受挫。
    Frustrated,
    /// 紧急。
    Urgent,
    /// 未知。
    Unknown,
}

impl UserAffect {
    /// 是否属于需要收紧表达的状态。
    #[must_use]
    pub const fn is_distressed(self) -> bool {
        matches!(self, Self::Frustrated | Self::Urgent)
    }
}

/// 已合成的表达指令（只读视图）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaDirective {
    /// 模式。
    pub mode: PersonaMode,
    /// 温度，0 到 100（已含状态偏移与时段收敛）。
    pub warmth: u8,
    /// 正式度，0 到 100（已含模式与规则）。
    pub formality: u8,
    /// 幽默档位。
    pub humor: HumorLevel,
    /// 朗读详略。
    pub verbosity: Verbosity,
    /// 是否允许一句久别致意。
    pub allow_reunion_greeting: bool,
    /// 本轮变化原因，用于诊断，不上屏。
    pub cause: String,
    /// 待纠偏提示，群聊场景已脱敏。
    pub guard: Option<String>,
}

/// 语音侧仍需要的人格状态字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersonaStateView {
    /// 用户情绪。
    pub user_affect: UserAffect,
}

/// 只读人格快照。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaSnapshot {
    /// 已合成的表达指令。
    pub directive: PersonaDirective,
    /// 状态视图。
    pub state: PersonaStateView,
}

impl PersonaSnapshot {
    /// 人格不可用时的中性快照。
    #[must_use]
    pub fn neutral() -> Self {
        Self {
            directive: PersonaDirective {
                mode: PersonaMode::Casual,
                warmth: 50,
                formality: 60,
                humor: HumorLevel::Subtle,
                verbosity: Verbosity::Concise,
                allow_reunion_greeting: false,
                cause: "人格快照不可用".to_owned(),
                guard: None,
            },
            state: PersonaStateView {
                user_affect: UserAffect::Unknown,
            },
        }
    }

    /// 是否处于群聊。
    #[must_use]
    pub fn is_group(&self) -> bool {
        matches!(self.directive.mode, PersonaMode::Group)
    }
}

#[cfg(test)]
mod tests {
    use super::{HumorLevel, PersonaMode, PersonaSnapshot, UserAffect};
    use crate::domain::profile::Verbosity;

    #[test]
    fn neutral_snapshot_is_safe_default() {
        let snapshot = PersonaSnapshot::neutral();
        assert_eq!(snapshot.directive.mode, PersonaMode::Casual);
        assert_eq!(snapshot.directive.verbosity, Verbosity::Concise);
        assert_eq!(snapshot.directive.humor, HumorLevel::Subtle);
        assert!(!snapshot.directive.allow_reunion_greeting);
        assert!(!snapshot.is_group());
        assert_eq!(snapshot.state.user_affect, UserAffect::Unknown);
    }

    #[test]
    fn distressed_states_are_detected() {
        assert!(UserAffect::Frustrated.is_distressed());
        assert!(UserAffect::Urgent.is_distressed());
        assert!(!UserAffect::Positive.is_distressed());
        assert!(!UserAffect::Unknown.is_distressed());
    }

    #[test]
    fn group_mode_is_reported() {
        let mut snapshot = PersonaSnapshot::neutral();
        snapshot.directive.mode = PersonaMode::Group;
        assert!(snapshot.is_group());
    }
}
