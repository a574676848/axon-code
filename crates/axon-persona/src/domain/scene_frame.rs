//! 场景帧与人格模式。
//!
//! 场景帧只承载轮前可确定性取得的信号，不含任何模型推断，因此零延迟、可复现。

use serde::{Deserialize, Serialize};

use crate::{constants::INCIDENT_FAILURE_STREAK, domain::persona_state::PersonaState};

/// 输入渠道。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    /// 桌面端。
    Desktop,
    /// 终端 TUI。
    Tui,
    /// 一次性 CLI 调用。
    Cli,
    /// 飞书远控。
    Feishu,
    /// 其余渠道。
    Other,
}

impl Channel {
    /// 按渠道标识解析；未知标识归入 [`Channel::Other`]。
    #[must_use]
    pub fn parse(token: &str) -> Self {
        match token.trim().to_ascii_lowercase().as_str() {
            "desktop" => Self::Desktop,
            "tui" => Self::Tui,
            "cli" => Self::Cli,
            "feishu" => Self::Feishu,
            _ => Self::Other,
        }
    }

    /// 返回规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Tui => "tui",
            Self::Cli => "cli",
            Self::Feishu => "feishu",
            Self::Other => "other",
        }
    }
}

/// 受众类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audience {
    /// 一对一。
    Private,
    /// 群聊。
    Group,
}

impl Audience {
    /// 返回规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Group => "group",
        }
    }

    /// 是否群聊。
    #[must_use]
    pub const fn is_group(self) -> bool {
        matches!(self, Self::Group)
    }
}

/// 一天中的时段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Daypart {
    /// 深夜，0 点至 5 点。
    LateNight,
    /// 上午，6 点至 11 点。
    Morning,
    /// 下午，12 点至 17 点。
    Afternoon,
    /// 晚间，18 点至 23 点。
    Evening,
}

impl Daypart {
    /// 按本地小时推导时段，超出 0 至 23 的值按取模折算。
    #[must_use]
    pub const fn from_local_hour(hour: u8) -> Self {
        match hour % 24 {
            0..=5 => Self::LateNight,
            6..=11 => Self::Morning,
            12..=17 => Self::Afternoon,
            _ => Self::Evening,
        }
    }

    /// 返回规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::LateNight => "late_night",
            Self::Morning => "morning",
            Self::Afternoon => "afternoon",
            Self::Evening => "evening",
        }
    }
}

/// 人格模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonaMode {
    /// 专注执行：精度优先。
    Focus,
    /// 休闲交流：允许温度。
    Casual,
    /// 故障或受挫：收敛幽默与篇幅。
    Incident,
    /// 群聊：正式、克制、不暴露私聊推断。
    Group,
}

impl PersonaMode {
    /// 返回规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Focus => "focus",
            Self::Casual => "casual",
            Self::Incident => "incident",
            Self::Group => "group",
        }
    }
}

/// 轮前场景帧。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneFrame {
    /// 当前渠道。
    pub channel: Channel,
    /// 当前受众。
    pub audience: Audience,
    /// 当前时段。
    pub daypart: Daypart,
    /// 是否存在 Workspace。
    pub has_workspace: bool,
    /// 当前场域：有工作区为办公，否则为生活。
    pub domain: PersonaDomain,
    /// 与上一轮结算之间的空闲毫秒数。
    pub idle_gap_ms: u64,
}

impl SceneFrame {
    /// 构造场景帧。
    #[must_use]
    pub const fn new(
        channel: Channel,
        audience: Audience,
        daypart: Daypart,
        has_workspace: bool,
        idle_gap_ms: u64,
    ) -> Self {
        Self {
            channel,
            audience,
            daypart,
            has_workspace,
            domain: if has_workspace {
                PersonaDomain::Office
            } else {
                PersonaDomain::DailyLife
            },
            idle_gap_ms,
        }
    }
}

/// 按优先级推导人格模式，首条命中生效。
///
/// 顺序固定为：群聊、故障或受挫、有 Workspace、其余。
#[must_use]
pub fn resolve_mode(state: &PersonaState, scene: &SceneFrame) -> PersonaMode {
    if scene.audience.is_group() {
        return PersonaMode::Group;
    }
    if state.failure_streak >= INCIDENT_FAILURE_STREAK || state.user_affect.is_distressed() {
        return PersonaMode::Incident;
    }
    if scene.has_workspace {
        return PersonaMode::Focus;
    }
    PersonaMode::Casual
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::persona_state::UserAffect;

    fn scene(audience: Audience, has_workspace: bool) -> SceneFrame {
        SceneFrame::new(Channel::Cli, audience, Daypart::Afternoon, has_workspace, 0)
    }

    fn state_with(affect: UserAffect, failure_streak: u8) -> PersonaState {
        PersonaState {
            user_affect: affect,
            failure_streak,
            ..PersonaState::baseline(0)
        }
    }

    #[test]
    fn daypart_boundaries_are_exact() {
        assert_eq!(Daypart::from_local_hour(0), Daypart::LateNight);
        assert_eq!(Daypart::from_local_hour(5), Daypart::LateNight);
        assert_eq!(Daypart::from_local_hour(6), Daypart::Morning);
        assert_eq!(Daypart::from_local_hour(11), Daypart::Morning);
        assert_eq!(Daypart::from_local_hour(12), Daypart::Afternoon);
        assert_eq!(Daypart::from_local_hour(17), Daypart::Afternoon);
        assert_eq!(Daypart::from_local_hour(18), Daypart::Evening);
        assert_eq!(Daypart::from_local_hour(23), Daypart::Evening);
        assert_eq!(Daypart::from_local_hour(24), Daypart::LateNight);
    }

    #[test]
    fn group_audience_wins_over_every_other_signal() {
        let state = state_with(UserAffect::Frustrated, 5);

        assert_eq!(
            resolve_mode(&state, &scene(Audience::Group, true)),
            PersonaMode::Group
        );
    }

    #[test]
    fn repeated_failures_escalate_to_incident_before_workspace() {
        let state = state_with(UserAffect::Neutral, INCIDENT_FAILURE_STREAK);

        assert_eq!(
            resolve_mode(&state, &scene(Audience::Private, true)),
            PersonaMode::Incident
        );
    }

    #[test]
    fn distressed_affect_escalates_without_failure_streak() {
        let state = state_with(UserAffect::Urgent, 0);

        assert_eq!(
            resolve_mode(&state, &scene(Audience::Private, false)),
            PersonaMode::Incident
        );
    }

    #[test]
    fn workspace_then_casual_are_the_remaining_tiers() {
        let calm = state_with(UserAffect::Positive, 0);

        assert_eq!(
            resolve_mode(&calm, &scene(Audience::Private, true)),
            PersonaMode::Focus
        );
        assert_eq!(
            resolve_mode(&calm, &scene(Audience::Private, false)),
            PersonaMode::Casual
        );
    }

    #[test]
    fn channel_tokens_round_trip_and_unknown_falls_to_other() {
        for channel in [
            Channel::Desktop,
            Channel::Tui,
            Channel::Cli,
            Channel::Feishu,
            Channel::Other,
        ] {
            assert_eq!(Channel::parse(channel.as_token()), channel);
        }
        assert_eq!(Channel::parse("CLI"), Channel::Cli);
        assert_eq!(Channel::parse("web"), Channel::Other);
    }
}

/// 场域：办公与生活两档，由是否存在工作区决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PersonaDomain {
    /// 办公：在工作区内处理代码、评审与交付。
    Office,
    /// 生活：在 Home 根处理日程、起居与日常事务。
    DailyLife,
}

impl PersonaDomain {
    /// 返回持久化与遥测使用的规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Office => "office",
            Self::DailyLife => "daily_life",
        }
    }

    /// 从规范记号解析。
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim() {
            "office" => Some(Self::Office),
            "daily_life" => Some(Self::DailyLife),
            _ => None,
        }
    }
}
