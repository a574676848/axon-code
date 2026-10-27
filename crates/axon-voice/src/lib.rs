//! Axon 语音能力的领域合同。
//!
//! 本 crate 只承载纯逻辑：相位状态机、失败分类、音色与韵律画像、配置模型与端口 trait。
//! 零 IO、零网络、零 FFI；模型推理、音频采集与协议编解码分别由 `axon-voice-host`、
//! `axon-voice-protocol` 与 daemon/desktop 的适配层承担。
//!
//! 合同的唯一事实源是合同仓库 `axon-voice-contract`（与本仓库同级的独立目录）：
//!
//! - 相位与合法迁移：`ARCHITECTURE.md` §3（32 条边）。
//! - 失败分类：`PROTOCOL.md` §4.2（21 个错误码 → 17 个变体）。
//! - 音色与韵律字段：`ARCHITECTURE.md` §9 与 WP-01/WP-14。

pub mod config;
pub mod constants;
pub mod domain;
pub mod expression;
pub mod fusion;
pub mod pitch;
pub mod port;
pub mod presets;
pub mod render;

pub use config::{
    AsrConfig, AudioConfig, ConfigError, MeetingMode, TtsConfig, TtsFallback, VoiceConfig,
    VoiceMode, VoicePersonaConfig, WorkerConfig,
};
pub use constants::{PITCH_MAX_SEMITONES, PITCH_MIN_SEMITONES, RATE_MAX, RATE_MIN};
pub use domain::{
    CueKind, HumorLevel, IdlePhase, ModelFamily, PersonaMode, PersonaSnapshot, Phase, PhaseError,
    PresetReason, ProfileError, ProfileSource, ProtocolReason, SpeechProfile, Stage,
    TransitionTrigger, UserAffect, Verbosity, VoiceFailure, WorkerSubStage,
};
pub use expression::{
    ExpressionStyle, SpeechSegment, parse_expression, render_with_expression, segments_with_pauses,
};
pub use fusion::{PersonaVoiceInput, VoicePlan, plan};
pub use port::{
    Clock, PcmChunk, SAMPLE_RATE_HZ, SpeechRenderer, SpeechToText, SttEvent, SttSession,
    TextToSpeech,
};
pub use presets::{PRESETS, VoicePreset, default_preset, resolve};
pub use render::{
    SpeechStream, render_for_speech, render_for_speech_with_override, render_sentences,
    render_sentences_with_override,
};
