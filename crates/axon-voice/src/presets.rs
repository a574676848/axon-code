//! 音色预设（WP-13）：五个名字只调音色与节奏，不做角色表演。
//!
//! `sweet-taiwan` 需要单独模型，未安装时标记为不可用，不冒充成别的音色。

use crate::domain::ProfileSource;
use crate::domain::profile::{SpeechProfile, Verbosity};

/// 预设。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoicePreset {
    /// 稳定 id。
    pub id: &'static str,
    /// 设置页显示名。
    pub name: &'static str,
    /// 说话人编号。
    pub speaker: u32,
    /// 语速。
    pub rate: f32,
    /// 音高（半音）。
    pub pitch_semitones: f32,
    /// 需要的模型目录名；`None` 表示当前主模型即可。
    pub required_model: Option<&'static str>,
    /// 是否默认。
    pub is_default: bool,
}

/// 全部预设。
pub const PRESETS: [VoicePreset; 5] = [
    VoicePreset {
        id: "calm-elegant",
        name: "冷静知性",
        speaker: 3,
        rate: 0.95,
        pitch_semitones: -1.0,
        required_model: None,
        is_default: true,
    },
    VoicePreset {
        id: "bright-neighbor",
        name: "明朗邻家",
        speaker: 7,
        rate: 1.05,
        pitch_semitones: 1.0,
        required_model: None,
        is_default: false,
    },
    VoicePreset {
        id: "playful-lively",
        name: "俏皮灵动",
        speaker: 11,
        rate: 1.08,
        pitch_semitones: 2.0,
        required_model: None,
        is_default: false,
    },
    VoicePreset {
        id: "warm-taiwan",
        name: "甜软台腔",
        speaker: 0,
        rate: 1.0,
        pitch_semitones: 0.0,
        required_model: Some("vits-zh-tw-soft"),
        is_default: false,
    },
    VoicePreset {
        id: "capable-assistant",
        name: "干练助理",
        speaker: 15,
        rate: 1.06,
        pitch_semitones: -0.5,
        required_model: None,
        is_default: false,
    },
];

/// 按 id 查找。
#[must_use]
pub fn resolve(id: &str) -> Option<VoicePreset> {
    PRESETS.iter().copied().find(|preset| preset.id == id)
}

/// 默认预设。
#[must_use]
pub fn default_preset() -> VoicePreset {
    PRESETS
        .iter()
        .copied()
        .find(|preset| preset.is_default)
        .unwrap_or(PRESETS[0])
}

impl VoicePreset {
    /// 是否可以在当前模型下使用。
    #[must_use]
    pub fn available(self) -> bool {
        self.required_model.is_none()
    }

    /// 落到合成画像；详略取人格派生值。
    #[must_use]
    pub fn to_profile(self, verbosity: Verbosity) -> SpeechProfile {
        SpeechProfile {
            speaker: self.speaker,
            rate: self.rate,
            pitch_semitones: self.pitch_semitones,
            pause_scale: 1.0,
            energy_db: 0.0,
            sentence_tail_semitones: 0.0,
            verbosity,
            source: ProfileSource::Settings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PRESETS, default_preset, resolve};
    use crate::domain::profile::Verbosity;

    #[test]
    fn has_five_presets_with_unique_ids() {
        assert_eq!(PRESETS.len(), 5);
        let mut ids: Vec<&str> = PRESETS.iter().map(|preset| preset.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn default_is_calm_elegant() {
        assert_eq!(default_preset().id, "calm-elegant");
        assert!(default_preset().is_default);
    }

    #[test]
    fn taiwan_requires_extra_model() {
        let preset = resolve("warm-taiwan").expect("存在");
        assert!(!preset.available());
        assert_eq!(preset.required_model, Some("vits-zh-tw-soft"));
    }

    #[test]
    fn profile_carries_speaker_and_rate() {
        let preset = resolve("capable-assistant").expect("存在");
        let profile = preset.to_profile(Verbosity::Concise);
        assert_eq!(profile.speaker, 15);
        assert!((profile.rate - 1.06).abs() < f32::EPSILON);
        assert!((profile.pitch_semitones + 0.5).abs() < f32::EPSILON);
        assert_eq!(profile.verbosity, Verbosity::Concise);
    }
}
