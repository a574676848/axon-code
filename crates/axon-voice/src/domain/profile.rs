//! 音色与韵律画像：唯一合并结果。
//!
//! 派生顺序固定为 `人格派生 → 预设覆盖音色四字段 → 用户设置`，合并结果只存在这里，
//! 播放层不得再叠加偏移。

use crate::constants::{
    ENERGY_DB_MAX, ENERGY_DB_MIN, PAUSE_SCALE_MAX, PAUSE_SCALE_MIN, PITCH_MAX_SEMITONES,
    PITCH_MIN_SEMITONES, RATE_MAX, RATE_MIN, TAIL_MAX_SEMITONES, TAIL_MIN_SEMITONES,
};

/// 朗读详略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verbosity {
    /// 只读结论。
    Minimal,
    /// 简洁。
    Concise,
    /// 详细。
    Detailed,
}

impl Verbosity {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Concise => "concise",
            Self::Detailed => "detailed",
        }
    }
}

/// 画像来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProfileSource {
    /// 用户设置选中了预设。
    Settings,
    /// 人格派生。
    Persona,
    /// 人格快照缺失时的兜底。
    Builtin,
}

impl ProfileSource {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Persona => "persona",
            Self::Builtin => "builtin",
        }
    }
}

/// 画像校验失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileError {
    /// 字段越界。
    OutOfRange {
        /// 字段名。
        field: &'static str,
    },
}

/// 音色与韵律画像。
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechProfile {
    /// 模型内说话人 id。
    pub speaker: u32,
    /// 语速倍率，0.85 到 1.15。
    pub rate: f32,
    /// 音高偏移（半音），-3.0 到 +3.0。
    pub pitch_semitones: f32,
    /// 句间停顿缩放，0.8 到 1.4。
    pub pause_scale: f32,
    /// 音量增益（dB），-3.0 到 +1.0。
    pub energy_db: f32,
    /// 句尾音高偏移（半音），-1.0 到 +1.0。
    pub sentence_tail_semitones: f32,
    /// 朗读详略，只来自人格。
    pub verbosity: Verbosity,
    /// 来源。
    pub source: ProfileSource,
}

impl SpeechProfile {
    /// 人格快照缺失时的兜底画像。
    #[must_use]
    pub const fn fallback() -> Self {
        Self {
            speaker: 0,
            rate: 1.0,
            pitch_semitones: 0.0,
            pause_scale: 1.0,
            energy_db: 0.0,
            sentence_tail_semitones: 0.0,
            verbosity: Verbosity::Concise,
            source: ProfileSource::Builtin,
        }
    }

    /// 校验所有数值字段是否落在契约区间内。
    ///
    /// # Errors
    ///
    /// 任一字段越界时返回 [`ProfileError::OutOfRange`]。
    pub fn validate(&self) -> Result<(), ProfileError> {
        check("rate", self.rate, RATE_MIN, RATE_MAX)?;
        check(
            "pitch_semitones",
            self.pitch_semitones,
            PITCH_MIN_SEMITONES,
            PITCH_MAX_SEMITONES,
        )?;
        check(
            "pause_scale",
            self.pause_scale,
            PAUSE_SCALE_MIN,
            PAUSE_SCALE_MAX,
        )?;
        check("energy_db", self.energy_db, ENERGY_DB_MIN, ENERGY_DB_MAX)?;
        check(
            "sentence_tail_semitones",
            self.sentence_tail_semitones,
            TAIL_MIN_SEMITONES,
            TAIL_MAX_SEMITONES,
        )?;
        Ok(())
    }
}

fn check(field: &'static str, value: f32, min: f32, max: f32) -> Result<(), ProfileError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(ProfileError::OutOfRange { field })
    }
}

#[cfg(test)]
mod tests {
    use super::{ProfileError, ProfileSource, SpeechProfile, Verbosity};

    #[test]
    fn fallback_is_within_range() {
        let profile = SpeechProfile::fallback();
        assert_eq!(profile.validate(), Ok(()));
        assert_eq!(profile.source, ProfileSource::Builtin);
        assert_eq!(profile.verbosity, Verbosity::Concise);
    }

    #[test]
    fn out_of_range_fields_are_rejected() {
        let mut profile = SpeechProfile::fallback();
        profile.rate = 1.16;
        assert_eq!(
            profile.validate(),
            Err(ProfileError::OutOfRange { field: "rate" })
        );

        let mut profile = SpeechProfile::fallback();
        profile.pause_scale = 0.79;
        assert_eq!(
            profile.validate(),
            Err(ProfileError::OutOfRange {
                field: "pause_scale"
            })
        );

        let mut profile = SpeechProfile::fallback();
        profile.energy_db = f32::NAN;
        assert_eq!(
            profile.validate(),
            Err(ProfileError::OutOfRange { field: "energy_db" })
        );
    }

    #[test]
    fn boundary_values_are_accepted() {
        let mut profile = SpeechProfile::fallback();
        profile.rate = 1.15;
        profile.pitch_semitones = -3.0;
        profile.pause_scale = 1.4;
        profile.energy_db = 1.0;
        profile.sentence_tail_semitones = -1.0;
        assert_eq!(profile.validate(), Ok(()));
    }
}
