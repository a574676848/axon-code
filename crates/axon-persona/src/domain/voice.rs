//! 表达基线：温度、正式度、幽默与篇幅。
//!
//! 这是一组可计算的表达参数，人格状态只允许在其上做有界偏移，不允许改变身份与底色。

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

impl HumorLevel {
    /// 返回 `SOUL.md` 使用的规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Subtle => "subtle",
            Self::Playful => "playful",
        }
    }

    /// 按规范记号解析；未知记号返回 `None`。
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim() {
            "off" => Some(Self::Off),
            "subtle" => Some(Self::Subtle),
            "playful" => Some(Self::Playful),
            _ => None,
        }
    }

    /// 降一档，用于精度优先或群聊场景。
    #[must_use]
    pub const fn downgrade(self) -> Self {
        match self {
            Self::Playful => Self::Subtle,
            Self::Subtle | Self::Off => Self::Off,
        }
    }
}

/// 篇幅档位，按 `Minimal`、`Concise`、`Detailed` 由短到长排列。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verbosity {
    /// 极简，只给结论。
    Minimal,
    /// 简洁。
    Concise,
    /// 详细。
    Detailed,
}

impl Verbosity {
    /// 返回 `SOUL.md` 使用的规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Concise => "concise",
            Self::Detailed => "detailed",
        }
    }

    /// 按规范记号解析；未知记号返回 `None`。
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token.trim() {
            "minimal" => Some(Self::Minimal),
            "concise" => Some(Self::Concise),
            "detailed" => Some(Self::Detailed),
            _ => None,
        }
    }

    /// 取更短的一档，用于上界收敛。
    #[must_use]
    pub const fn cap(self, upper: Self) -> Self {
        if (self as u8) <= (upper as u8) {
            self
        } else {
            upper
        }
    }
}

/// 表达参数集合。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Voice {
    /// 温度，取值 0 至 100。
    pub warmth: u8,
    /// 正式度，取值 0 至 100。
    pub formality: u8,
    /// 幽默档位。
    pub humor: HumorLevel,
    /// 篇幅档位。
    pub verbosity: Verbosity,
}

impl Voice {
    /// 构造表达参数，数值越界时收敛到 0 至 100。
    #[must_use]
    pub const fn new(warmth: u8, formality: u8, humor: HumorLevel, verbosity: Verbosity) -> Self {
        Self {
            warmth: clamp_percent(warmth),
            formality: clamp_percent(formality),
            humor,
            verbosity,
        }
    }

    /// 按补丁覆盖字段，未给出的字段保持原值。
    #[must_use]
    pub fn apply(self, patch: VoicePatch) -> Self {
        Self {
            warmth: patch.warmth.map_or(self.warmth, clamp_percent),
            formality: patch.formality.map_or(self.formality, clamp_percent),
            humor: patch.humor.unwrap_or(self.humor),
            verbosity: patch.verbosity.unwrap_or(self.verbosity),
        }
    }

    /// 叠加温度增量并按 0 至 100 收敛。
    #[must_use]
    pub fn with_warmth_delta(self, delta: i8) -> Self {
        let warmth = i16::from(self.warmth) + i16::from(delta);
        Self {
            warmth: clamp_percent(u8::try_from(warmth.clamp(0, 100)).unwrap_or(0)),
            ..self
        }
    }

    /// 叠加正式度增量并按 0 至 100 收敛。
    #[must_use]
    pub fn with_formality_delta(self, delta: i8) -> Self {
        let formality = i16::from(self.formality) + i16::from(delta);
        Self {
            formality: clamp_percent(u8::try_from(formality.clamp(0, 100)).unwrap_or(0)),
            ..self
        }
    }

    /// 渲染单行表达描述，用于稳定段与动态段。
    #[must_use]
    pub fn render(&self) -> String {
        format!(
            "warmth={} formality={} humor={} verbosity={}",
            self.warmth,
            self.formality,
            self.humor.as_token(),
            self.verbosity.as_token()
        )
    }
}

/// 表达参数补丁；`None` 表示不覆盖。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VoicePatch {
    /// 温度。
    pub warmth: Option<u8>,
    /// 正式度。
    pub formality: Option<u8>,
    /// 幽默档位。
    pub humor: Option<HumorLevel>,
    /// 篇幅档位。
    pub verbosity: Option<Verbosity>,
}

impl VoicePatch {
    /// 是否没有任何字段。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.warmth.is_none()
            && self.formality.is_none()
            && self.humor.is_none()
            && self.verbosity.is_none()
    }
}

const fn clamp_percent(value: u8) -> u8 {
    if value > 100 { 100 } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice() -> Voice {
        Voice::new(40, 60, HumorLevel::Subtle, Verbosity::Concise)
    }

    #[test]
    fn apply_only_overwrites_given_fields() {
        let patched = voice().apply(VoicePatch {
            humor: Some(HumorLevel::Off),
            ..VoicePatch::default()
        });

        assert_eq!(patched.humor, HumorLevel::Off);
        assert_eq!(patched.warmth, 40);
        assert_eq!(patched.formality, 60);
        assert_eq!(patched.verbosity, Verbosity::Concise);
    }

    #[test]
    fn warmth_delta_clamps_at_both_ends() {
        let hot = Voice::new(99, 50, HumorLevel::Off, Verbosity::Concise).with_warmth_delta(5);
        let cold = Voice::new(1, 50, HumorLevel::Off, Verbosity::Concise).with_warmth_delta(-9);

        assert_eq!(hot.warmth, 100);
        assert_eq!(cold.warmth, 0);
    }

    #[test]
    fn humor_downgrade_bottoms_out_at_off() {
        assert_eq!(HumorLevel::Playful.downgrade(), HumorLevel::Subtle);
        assert_eq!(HumorLevel::Subtle.downgrade(), HumorLevel::Off);
        assert_eq!(HumorLevel::Off.downgrade(), HumorLevel::Off);
    }

    #[test]
    fn verbosity_cap_keeps_the_shorter_upper_bound() {
        assert_eq!(
            Verbosity::Detailed.cap(Verbosity::Concise),
            Verbosity::Concise
        );
        assert_eq!(
            Verbosity::Minimal.cap(Verbosity::Concise),
            Verbosity::Minimal
        );
    }

    #[test]
    fn tokens_round_trip_and_unknown_tokens_are_rejected() {
        for humor in [HumorLevel::Off, HumorLevel::Subtle, HumorLevel::Playful] {
            assert_eq!(HumorLevel::parse(humor.as_token()), Some(humor));
        }
        for verbosity in [Verbosity::Minimal, Verbosity::Concise, Verbosity::Detailed] {
            assert_eq!(Verbosity::parse(verbosity.as_token()), Some(verbosity));
        }
        assert_eq!(HumorLevel::parse("loud"), None);
        assert_eq!(Verbosity::parse("verbose"), None);
    }

    #[test]
    fn render_is_canonical_and_stable() {
        assert_eq!(
            voice().render(),
            "warmth=40 formality=60 humor=subtle verbosity=concise"
        );
        assert_eq!(voice().render(), voice().render());
    }
}
