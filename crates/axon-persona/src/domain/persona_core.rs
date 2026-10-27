//! 人格内核：身份、来处、不可违背项、特质、表达基线与场景规则。

use crate::domain::{
    scene_frame::{Audience, PersonaMode},
    voice::{Voice, VoicePatch},
};

/// 身份三要素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// 角色名称。
    pub name: String,
    /// 默认称呼。
    pub address: String,
    /// 角色定位。
    pub role: String,
}

/// 场景规则的条件。
///
/// 只允许 `mode` 与 `audience` 两个键，避免把任意条件塞进人格内核。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuleCondition {
    /// 命中的模式。
    pub mode: Option<PersonaMode>,
    /// 命中的受众。
    pub audience: Option<Audience>,
}

impl RuleCondition {
    /// 条件是否为空；空条件在解析阶段即被拒绝。
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.mode.is_none() && self.audience.is_none()
    }

    /// 判断当前模式与受众是否命中本条件。
    #[must_use]
    pub fn matches(&self, mode: PersonaMode, audience: Audience) -> bool {
        self.mode.is_none_or(|expected| expected == mode)
            && self.audience.is_none_or(|expected| expected == audience)
    }
}

/// 一条场景规则。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SceneRule {
    /// 命中条件。
    pub when: RuleCondition,
    /// 命中后覆盖的表达参数。
    pub voice: VoicePatch,
}

/// 人格内核。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaCore {
    /// 身份三要素。
    pub identity: Identity,
    /// 来处与经历，构成身份深度。
    pub backstory: String,
    /// 不可违背项，优先级高于一切状态与规则。
    pub invariants: Vec<String>,
    /// 核心特质。
    pub traits: Vec<String>,
    /// 表达基线。
    pub voice: Voice,
    /// 场景规则，按声明顺序匹配，首条命中生效。
    pub rules: Vec<SceneRule>,
    /// 正文补充说明。
    pub notes: Option<String>,
}

impl PersonaCore {
    /// 返回首条命中的场景规则。
    #[must_use]
    pub fn first_matching_rule(&self, mode: PersonaMode, audience: Audience) -> Option<&SceneRule> {
        self.rules
            .iter()
            .find(|rule| rule.when.matches(mode, audience))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        mode: Option<PersonaMode>,
        humor: Option<crate::domain::voice::HumorLevel>,
    ) -> SceneRule {
        SceneRule {
            when: RuleCondition {
                mode,
                audience: None,
            },
            voice: VoicePatch {
                humor,
                ..VoicePatch::default()
            },
        }
    }

    #[test]
    fn condition_requires_at_least_one_key() {
        assert!(RuleCondition::default().is_empty());
        assert!(
            !RuleCondition {
                mode: Some(PersonaMode::Focus),
                audience: None,
            }
            .is_empty()
        );
    }

    #[test]
    fn condition_matches_only_declared_keys() {
        let mode_only = RuleCondition {
            mode: Some(PersonaMode::Incident),
            audience: None,
        };
        assert!(mode_only.matches(PersonaMode::Incident, Audience::Group));
        assert!(!mode_only.matches(PersonaMode::Focus, Audience::Private));

        let audience_only = RuleCondition {
            mode: None,
            audience: Some(Audience::Group),
        };
        assert!(audience_only.matches(PersonaMode::Focus, Audience::Group));
        assert!(!audience_only.matches(PersonaMode::Focus, Audience::Private));
    }

    #[test]
    fn first_matching_rule_wins_in_declaration_order() {
        let core = PersonaCore {
            identity: Identity {
                name: "Axon".to_owned(),
                address: "长官".to_owned(),
                role: "副官".to_owned(),
            },
            backstory: "来处".to_owned(),
            invariants: vec!["依据事实作答".to_owned()],
            traits: vec!["沉稳".to_owned()],
            voice: Voice::new(
                40,
                60,
                crate::domain::voice::HumorLevel::Playful,
                crate::domain::voice::Verbosity::Concise,
            ),
            rules: vec![
                rule(
                    Some(PersonaMode::Focus),
                    Some(crate::domain::voice::HumorLevel::Off),
                ),
                rule(None, Some(crate::domain::voice::HumorLevel::Playful)),
            ],
            notes: None,
        };

        let first = core
            .first_matching_rule(PersonaMode::Focus, Audience::Private)
            .expect("应命中首条规则");
        assert_eq!(
            first.voice.humor,
            Some(crate::domain::voice::HumorLevel::Off)
        );

        assert!(
            core.first_matching_rule(PersonaMode::Casual, Audience::Private)
                .is_some(),
            "无 mode 限定的规则应作为兜底命中"
        );
    }
}
