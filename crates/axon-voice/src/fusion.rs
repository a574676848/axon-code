//! 人格与音色的融合：语音不是另一套人格，只是同一份表达参数的声学映射。
//!
//! 输入是人格侧已合成的表达参数（模式、温度、正式度、幽默、详略），
//! 输出是预设选择与合成画像。显式音色预设只覆盖音色四字段，详略始终听人格的。

use crate::domain::ProfileSource;
use crate::domain::profile::{SpeechProfile, Verbosity};
use crate::domain::snapshot::{HumorLevel, PersonaMode};
use crate::presets::{self, VoicePreset};

/// 人格侧输入（与 `axon-persona` 的 `Voice` 同语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PersonaVoiceInput {
    /// 模式。
    pub mode: PersonaMode,
    /// 温度，0 到 100。
    pub warmth: u8,
    /// 正式度，0 到 100。
    pub formality: u8,
    /// 幽默档位。
    pub humor: HumorLevel,
    /// 详略。
    pub verbosity: Verbosity,
}

impl Default for PersonaVoiceInput {
    fn default() -> Self {
        Self {
            mode: PersonaMode::Casual,
            warmth: 50,
            formality: 60,
            humor: HumorLevel::Subtle,
            verbosity: Verbosity::Concise,
        }
    }
}

/// 一次朗读的融合结果。
#[derive(Debug, Clone, PartialEq)]
pub struct VoicePlan {
    /// 选中的音色预设。
    pub preset: VoicePreset,
    /// 合成画像。
    pub profile: SpeechProfile,
    /// 是否由人格推导（而非设置覆盖）。
    pub from_persona: bool,
}

/// 融合人格与可选预设覆盖，并按小时应用深夜基线。
#[must_use]
pub fn plan(
    input: Option<PersonaVoiceInput>,
    preset_override: Option<VoicePreset>,
    hour: u8,
) -> VoicePlan {
    let persona = input.unwrap_or_default();
    let from_persona = preset_override.is_none();
    let preset = preset_override.unwrap_or_else(|| choose_preset(persona));
    let mut profile = preset.profile_for(persona.verbosity, hour);

    let warmth = (f32::from(persona.warmth) - 50.0) / 50.0;
    let formality = (f32::from(persona.formality) - 50.0) / 50.0;
    profile.rate += warmth * 0.03 - formality * 0.02;
    profile.energy_db += warmth * 1.2;
    profile.pitch_semitones += warmth * 0.4;
    profile.pause_scale += formality * 0.12;

    match persona.mode {
        PersonaMode::Focus => profile.pause_scale += 0.05,
        PersonaMode::Incident => {
            profile.rate += 0.03;
            profile.pause_scale -= 0.05;
        }
        PersonaMode::Group => {
            profile.energy_db -= 1.0;
            profile.rate -= 0.02;
        }
        PersonaMode::Casual => {}
    }
    if persona.humor == HumorLevel::Playful {
        profile.pitch_semitones += 0.3;
        profile.rate += 0.02;
    }

    profile.rate = profile.rate.clamp(0.85, 1.15);
    profile.pitch_semitones = profile.pitch_semitones.clamp(-3.0, 3.0);
    profile.energy_db = profile.energy_db.clamp(-3.0, 1.0);
    profile.pause_scale = profile.pause_scale.clamp(0.8, 1.4);
    profile.source = if from_persona {
        ProfileSource::Persona
    } else {
        ProfileSource::Settings
    };

    VoicePlan {
        preset,
        profile,
        from_persona,
    }
}

fn choose_preset(persona: PersonaVoiceInput) -> VoicePreset {
    let by_humor = matches!(persona.humor, HumorLevel::Playful);
    let id = match persona.mode {
        PersonaMode::Group | PersonaMode::Incident => "capable-assistant",
        PersonaMode::Focus => "calm-elegant",
        PersonaMode::Casual => {
            if by_humor && persona.warmth >= 60 {
                "playful-lively"
            } else if persona.warmth >= 60 {
                "bright-neighbor"
            } else {
                "calm-elegant"
            }
        }
    };
    presets::resolve(id).unwrap_or_else(presets::default_preset)
}

#[cfg(test)]
mod tests {
    use super::{PersonaVoiceInput, plan};
    use crate::domain::profile::Verbosity;
    use crate::domain::snapshot::{HumorLevel, PersonaMode};
    use crate::presets::resolve;

    fn persona(
        mode: PersonaMode,
        warmth: u8,
        formality: u8,
        humor: HumorLevel,
    ) -> PersonaVoiceInput {
        PersonaVoiceInput {
            mode,
            warmth,
            formality,
            humor,
            verbosity: Verbosity::Concise,
        }
    }

    #[test]
    fn group_and_incident_prefer_capable_voice() {
        for mode in [PersonaMode::Group, PersonaMode::Incident] {
            let plan = plan(Some(persona(mode, 50, 70, HumorLevel::Subtle)), None, 14);
            assert_eq!(plan.preset.id, "capable-assistant");
            assert!(plan.from_persona);
        }
    }

    #[test]
    fn casual_warm_playful_gets_playful_voice() {
        let plan = plan(
            Some(persona(PersonaMode::Casual, 80, 40, HumorLevel::Playful)),
            None,
            14,
        );
        assert_eq!(plan.preset.id, "playful-lively");
    }

    #[test]
    fn casual_cool_gets_calm_voice() {
        let plan = plan(
            Some(persona(PersonaMode::Casual, 30, 60, HumorLevel::Subtle)),
            None,
            14,
        );
        assert_eq!(plan.preset.id, "calm-elegant");
    }

    #[test]
    fn warmth_and_formality_shift_profile() {
        let cool = plan(
            Some(persona(PersonaMode::Casual, 10, 90, HumorLevel::Off)),
            None,
            14,
        );
        let warm = plan(
            Some(persona(PersonaMode::Casual, 90, 10, HumorLevel::Off)),
            None,
            14,
        );
        assert!(warm.profile.rate > cool.profile.rate);
        assert!(warm.profile.energy_db > cool.profile.energy_db);
        assert!(warm.profile.pause_scale < cool.profile.pause_scale);
    }

    #[test]
    fn preset_override_keeps_speaker_and_persona_keeps_verbosity() {
        let preset = resolve("capable-assistant").expect("存在");
        let mut input = persona(PersonaMode::Group, 50, 80, HumorLevel::Off);
        input.verbosity = Verbosity::Detailed;
        let plan = plan(Some(input), Some(preset), 14);
        assert_eq!(plan.profile.speaker, 15);
        assert_eq!(plan.profile.verbosity, Verbosity::Detailed);
        assert!(!plan.from_persona);
    }

    #[test]
    fn missing_persona_falls_back_to_default_baseline() {
        let plan = plan(None, None, 14);
        assert_eq!(plan.preset.id, "calm-elegant");
        assert_eq!(plan.profile.verbosity, Verbosity::Concise);
    }

    #[test]
    fn late_night_lowers_energy_and_rate() {
        let day = plan(Some(PersonaVoiceInput::default()), None, 14);
        let night = plan(Some(PersonaVoiceInput::default()), None, 23);
        assert!(night.profile.rate < day.profile.rate);
        assert!(night.profile.energy_db < day.profile.energy_db);
    }

    #[test]
    fn profile_stays_inside_contract_bounds() {
        for warmth in [0u8, 50, 100] {
            for formality in [0u8, 50, 100] {
                let plan = plan(
                    Some(persona(
                        PersonaMode::Incident,
                        warmth,
                        formality,
                        HumorLevel::Playful,
                    )),
                    None,
                    23,
                );
                assert!((0.85..=1.15).contains(&plan.profile.rate));
                assert!((-3.0..=1.0).contains(&plan.profile.energy_db));
                assert!((0.8..=1.4).contains(&plan.profile.pause_scale));
            }
        }
    }
}
