//! 语音配置模型与校验。
//!
//! 字段与 `WP-10` 的骨架一一对应；采样率、采集帧长、合帧长度、VAD 输入形状属协议常量，
//! 不在这里出现。

/// 会话模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceMode {
    /// 按住说话。
    PushToTalk,
    /// 全双工。
    FullDuplex,
}

impl VoiceMode {
    /// 规范记号。
    #[must_use]
    pub const fn as_token(self) -> &'static str {
        match self {
            Self::PushToTalk => "push_to_talk",
            Self::FullDuplex => "full_duplex",
        }
    }
}

/// 系统音色兜底策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtsFallback {
    /// 允许回落系统音色。
    System,
    /// 不回落。
    None,
}

/// 会议模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingMode {
    /// 自动检测。
    Auto,
    /// 强制静音 cue。
    On,
    /// 关闭检测。
    Off,
}

/// 采集与打断参数。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioConfig {
    /// 预卷时长。
    pub pre_roll_ms: u32,
    /// 端点静音。
    pub endpoint_silence_ms: u32,
    /// VAD 起始帧数。
    pub vad_start_frames: u32,
    /// 打断判决块。
    pub barge_block_ms: u32,
    /// 底噪采样窗。
    pub barge_floor_window_ms: u32,
    /// 播放宽限。
    pub barge_grace_ms: u32,
    /// 判决窗。
    pub barge_window_ms: u32,
    /// 判决比例。
    pub barge_window_ratio: f32,
    /// 底噪倍数。
    pub barge_floor_multiplier: f32,
    /// 播放中阈值下限。
    pub barge_min_trigger: u32,
    /// 阈值上限。
    pub barge_max_trigger: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            pre_roll_ms: 1_200,
            endpoint_silence_ms: 500,
            vad_start_frames: 2,
            barge_block_ms: 30,
            barge_floor_window_ms: 450,
            barge_grace_ms: 500,
            barge_window_ms: 300,
            barge_window_ratio: 0.8,
            barge_floor_multiplier: 3.0,
            barge_min_trigger: 1_500,
            barge_max_trigger: 4_000,
        }
    }
}

/// 识别配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrConfig {
    /// 提供方。
    pub provider: String,
    /// 模型标识。
    pub model: String,
    /// 热词。
    pub hot_words: Vec<String>,
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            provider: "sherpa".to_owned(),
            model: "sherpa-streaming-zh-en".to_owned(),
            hot_words: Vec::new(),
        }
    }
}

/// 合成配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TtsConfig {
    /// 朗读总开关。
    pub enabled: bool,
    /// 文本轮是否也朗读。
    pub read_text_turns: bool,
    /// 音色预设 id，空串表示跟随人格。
    pub preset: String,
    /// 用户预设清单路径，空串表示用运行态根下的默认文件。
    pub presets_file: String,
    /// 提供方。
    pub provider: String,
    /// 模型标识。
    pub model: String,
    /// 兜底策略。
    pub fallback: TtsFallback,
    /// 分句最小字数。
    pub sentence_min_chars: u32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            read_text_turns: false,
            preset: String::new(),
            presets_file: String::new(),
            provider: "sherpa".to_owned(),
            model: "vits-zh".to_owned(),
            fallback: TtsFallback::System,
            sentence_min_chars: 6,
        }
    }
}

/// 人格化表现配置。
#[derive(Debug, Clone, PartialEq)]
pub struct VoicePersonaConfig {
    /// 总开关。
    pub enabled: bool,
    /// 回合交接音总开关。
    pub cues: bool,
    /// 收到回执。
    pub ack: bool,
    /// 思考音。
    pub thinking_sound: bool,
    /// 打断回执。
    pub interrupted_ack: bool,
    /// 久别致意。
    pub reunion: bool,
    /// 完成播报。
    pub done: bool,
    /// 错误回执。
    pub error_ack: bool,
    /// 会议模式。
    pub meeting_mode: MeetingMode,
    /// 预计首 token 超过此值才发 ack。
    pub ack_after_ms: u32,
    /// 进入思考后多久发思考音。
    pub thinking_after_ms: u32,
    /// 两次 ack 的最小间隔。
    pub ack_cooldown_ms: u32,
}

impl Default for VoicePersonaConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            cues: true,
            ack: true,
            thinking_sound: true,
            interrupted_ack: true,
            reunion: true,
            done: true,
            error_ack: true,
            meeting_mode: MeetingMode::Auto,
            ack_after_ms: 800,
            thinking_after_ms: 600,
            ack_cooldown_ms: 8_000,
        }
    }
}

/// 宿主配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerConfig {
    /// 空闲退出秒数。
    pub idle_timeout_seconds: u32,
    /// 自动重启上限。
    pub max_restart: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            idle_timeout_seconds: 600,
            max_restart: 3,
        }
    }
}

/// 顶层语音配置。
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceConfig {
    /// 总开关。
    pub enabled: bool,
    /// 会话模式。
    pub mode: VoiceMode,
    /// 语言提示。
    pub language: String,
    /// 打断时是否中断回合。
    pub interrupt_turn: bool,
    /// 并发上限，本期固定 1。
    pub concurrent_limit: u32,
    /// 采集参数。
    pub audio: AudioConfig,
    /// 识别配置。
    pub asr: AsrConfig,
    /// 合成配置。
    pub tts: TtsConfig,
    /// 人格化表现。
    pub persona: VoicePersonaConfig,
    /// 宿主配置。
    pub worker: WorkerConfig,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: VoiceMode::FullDuplex,
            language: "zh".to_owned(),
            interrupt_turn: true,
            concurrent_limit: 1,
            audio: AudioConfig::default(),
            asr: AsrConfig::default(),
            tts: TtsConfig::default(),
            persona: VoicePersonaConfig::default(),
            worker: WorkerConfig::default(),
        }
    }
}

/// 配置错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    /// 字段路径。
    pub field: &'static str,
    /// 原因。
    pub reason: &'static str,
}

impl VoiceConfig {
    /// 校验全部字段。
    ///
    /// # Errors
    ///
    /// 返回第一个越界或非法字段。
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.concurrent_limit != 1 {
            return Err(err("concurrent_limit", "本期固定为 1"));
        }
        if !matches!(self.language.as_str(), "zh" | "en" | "auto") {
            return Err(err("language", "只允许 zh、en、auto"));
        }
        let audio = &self.audio;
        range_u32("audio.pre_roll_ms", audio.pre_roll_ms, 600, 3_000)?;
        range_u32(
            "audio.endpoint_silence_ms",
            audio.endpoint_silence_ms,
            300,
            1_500,
        )?;
        range_u32("audio.vad_start_frames", audio.vad_start_frames, 1, 5)?;
        range_u32("audio.barge_block_ms", audio.barge_block_ms, 20, 40)?;
        range_u32(
            "audio.barge_floor_window_ms",
            audio.barge_floor_window_ms,
            200,
            1_000,
        )?;
        range_u32("audio.barge_grace_ms", audio.barge_grace_ms, 0, 1_000)?;
        range_u32("audio.barge_window_ms", audio.barge_window_ms, 200, 600)?;
        range_f32(
            "audio.barge_window_ratio",
            audio.barge_window_ratio,
            0.6,
            1.0,
        )?;
        range_f32(
            "audio.barge_floor_multiplier",
            audio.barge_floor_multiplier,
            2.0,
            6.0,
        )?;
        range_u32(
            "audio.barge_min_trigger",
            audio.barge_min_trigger,
            800,
            3_000,
        )?;
        range_u32(
            "audio.barge_max_trigger",
            audio.barge_max_trigger,
            3_000,
            8_000,
        )?;
        if audio.barge_min_trigger >= audio.barge_max_trigger {
            return Err(err("audio.barge_min_trigger", "必须小于 barge_max_trigger"));
        }
        range_u32("tts.sentence_min_chars", self.tts.sentence_min_chars, 1, 40)?;
        range_u32(
            "persona.ack_after_ms",
            self.persona.ack_after_ms,
            200,
            2_000,
        )?;
        range_u32(
            "persona.thinking_after_ms",
            self.persona.thinking_after_ms,
            100,
            2_000,
        )?;
        range_u32(
            "persona.ack_cooldown_ms",
            self.persona.ack_cooldown_ms,
            0,
            60_000,
        )?;
        range_u32(
            "worker.idle_timeout_seconds",
            self.worker.idle_timeout_seconds,
            60,
            3_600,
        )?;
        range_u32("worker.max_restart", self.worker.max_restart, 0, 10)?;
        if self.asr.model.trim().is_empty() || self.tts.model.trim().is_empty() {
            return Err(err("asr.model", "模型标识不能为空"));
        }
        Ok(())
    }
}

fn err(field: &'static str, reason: &'static str) -> ConfigError {
    ConfigError { field, reason }
}

fn range_u32(field: &'static str, value: u32, min: u32, max: u32) -> Result<(), ConfigError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(err(field, "取值越界"))
    }
}

fn range_f32(field: &'static str, value: f32, min: f32, max: f32) -> Result<(), ConfigError> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(err(field, "取值越界"))
    }
}

#[cfg(test)]
mod tests {
    use super::{ConfigError, MeetingMode, TtsFallback, VoiceConfig, VoiceMode};

    #[test]
    fn default_config_is_valid() {
        assert_eq!(VoiceConfig::default().validate(), Ok(()));
    }

    #[test]
    fn concurrent_limit_is_single_lane() {
        let config = VoiceConfig {
            concurrent_limit: 2,
            ..VoiceConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError {
                field: "concurrent_limit",
                reason: "本期固定为 1"
            })
        );
    }

    #[test]
    fn trigger_bounds_are_enforced() {
        let mut config = VoiceConfig::default();
        config.audio.barge_min_trigger = 400;
        assert!(config.validate().is_err());

        let mut config = VoiceConfig::default();
        config.audio.barge_max_trigger = 9_000;
        assert!(config.validate().is_err());

        let mut config = VoiceConfig::default();
        config.audio.barge_min_trigger = 3_000;
        config.audio.barge_max_trigger = 3_000;
        assert!(config.validate().is_err());
    }

    #[test]
    fn persona_timings_are_enforced() {
        let mut config = VoiceConfig::default();
        config.persona.ack_after_ms = 100;
        assert!(config.validate().is_err());
    }

    #[test]
    fn empty_model_is_rejected() {
        let mut config = VoiceConfig::default();
        config.tts.model = "  ".to_owned();
        assert!(config.validate().is_err());
    }

    #[test]
    fn defaults_match_contract() {
        let config = VoiceConfig::default();
        assert_eq!(config.mode, VoiceMode::FullDuplex);
        assert_eq!(config.audio.endpoint_silence_ms, 500);
        assert_eq!(config.audio.barge_min_trigger, 1_500);
        assert_eq!(config.audio.barge_max_trigger, 4_000);
        assert_eq!(config.tts.fallback, TtsFallback::System);
        assert_eq!(config.persona.meeting_mode, MeetingMode::Auto);
        assert!(!config.tts.read_text_turns);
    }
}
