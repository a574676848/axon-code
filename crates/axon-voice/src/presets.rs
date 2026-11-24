//! 音色预设（WP-13）：五个名字只调音色与节奏，不做角色表演。
//!
//! `sweet-taiwan` 需要单独模型，未安装时标记为不可用，不冒充成别的音色。
//! 每个预设自带角色口癖，供 cue 使用；深夜会降能量与降速（低语感）。

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
    /// 是否默认。
    pub is_default: bool,
    /// 思考时的口癖。
    pub thinking_text: &'static str,
    /// 收到回执时的口癖。
    pub ack_text: &'static str,
}

/// 全部预设。
pub const PRESETS: [VoicePreset; 4] = [
    VoicePreset {
        id: "calm-elegant",
        name: "冷静知性",
        speaker: 3,
        rate: 0.95,
        pitch_semitones: -1.0,
        is_default: true,
        thinking_text: "让我想想……",
        ack_text: "嗯，我在听",
    },
    VoicePreset {
        id: "bright-neighbor",
        name: "明朗邻家",
        speaker: 7,
        rate: 1.05,
        pitch_semitones: 1.0,
        is_default: false,
        thinking_text: "我看看哦",
        ack_text: "好嘞",
    },
    VoicePreset {
        id: "playful-lively",
        name: "俏皮灵动",
        speaker: 11,
        rate: 1.08,
        pitch_semitones: 2.0,
        is_default: false,
        thinking_text: "嗯哼，等我一下",
        ack_text: "来啦",
    },
    VoicePreset {
        id: "capable-assistant",
        name: "干练助理",
        speaker: 15,
        rate: 1.06,
        pitch_semitones: -0.5,
        is_default: false,
        thinking_text: "收到，处理中",
        ack_text: "明白",
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
    /// 内置音色均可用；保留能力探测口径。
    #[must_use]
    pub fn available(self) -> bool {
        true
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

    /// 按时段落地：深夜（22 点到 6 点）降能量、降速，形成低语感。
    #[must_use]
    pub fn profile_for(self, verbosity: Verbosity, hour: u8) -> SpeechProfile {
        let mut profile = self.to_profile(verbosity);
        if !(6..22).contains(&hour) {
            profile.rate = (profile.rate * 0.95).max(0.85);
            profile.energy_db = -2.0;
        }
        profile
    }
}

#[cfg(test)]
mod tests {
    use super::{PRESETS, default_preset, resolve};
    use crate::domain::profile::Verbosity;

    #[test]
    fn has_four_presets_with_unique_ids() {
        assert_eq!(PRESETS.len(), 4);
        let mut ids: Vec<&str> = PRESETS.iter().map(|preset| preset.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn default_is_calm_elegant() {
        assert_eq!(default_preset().id, "calm-elegant");
        assert!(default_preset().is_default);
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

    #[test]
    fn late_night_is_lower_and_slower() {
        let preset = default_preset();
        let day = preset.profile_for(Verbosity::Concise, 14);
        let night = preset.profile_for(Verbosity::Concise, 23);
        assert!(night.rate < day.rate);
        assert!((night.energy_db + 2.0).abs() < f32::EPSILON);
    }

    #[test]
    fn cues_are_character_specific() {
        assert_eq!(
            resolve("calm-elegant").expect("存在").thinking_text,
            "让我想想……"
        );
        assert_ne!(
            resolve("calm-elegant").expect("存在").ack_text,
            resolve("playful-lively").expect("存在").ack_text
        );
    }
}
