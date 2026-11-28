//! `axon setup voice` 参数定义。

use super::{Args, OutputFormat, SetupProviderProtocol, SetupWireTransport, ValueEnum};
use axon_config::voice::{
    VOICE_LANE_AUTO, VOICE_LANE_CASCADE, VOICE_LANE_REALTIME, VOICE_ROLE_ASR, VOICE_ROLE_REALTIME,
    VOICE_ROLE_TTS, VOICE_TURN_MANUAL, VOICE_TURN_SERVER_VAD, VOICE_TURN_SMART_TURN,
};

/// `axon setup voice` 语音车道取值。
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum VoiceLaneArg {
    /// 级联识别与合成。
    Cascade,
    /// 端到端实时语音。
    Realtime,
    /// 自动选择。
    Auto,
}

impl VoiceLaneArg {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::Cascade => VOICE_LANE_CASCADE,
            Self::Realtime => VOICE_LANE_REALTIME,
            Self::Auto => VOICE_LANE_AUTO,
        }
    }
}

/// `axon setup voice` 语音模型角色取值。
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum VoiceRoleArg {
    /// 语音识别。
    Asr,
    /// 语音合成。
    Tts,
    /// 端到端实时。
    Realtime,
}

impl VoiceRoleArg {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::Asr => VOICE_ROLE_ASR,
            Self::Tts => VOICE_ROLE_TTS,
            Self::Realtime => VOICE_ROLE_REALTIME,
        }
    }
}

/// `axon setup voice` 轮次检测策略取值。
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum VoiceTurnDetectionArg {
    /// 服务端 VAD。
    ServerVad,
    /// 语义轮次。
    SmartTurn,
    /// 手动按键说话。
    Manual,
}

impl VoiceTurnDetectionArg {
    pub(crate) const fn as_config_value(self) -> &'static str {
        match self {
            Self::ServerVad => VOICE_TURN_SERVER_VAD,
            Self::SmartTurn => VOICE_TURN_SMART_TURN,
            Self::Manual => VOICE_TURN_MANUAL,
        }
    }
}

/// `axon setup voice` 参数，可读写独立 `[voice]` 段。
#[derive(Debug, Clone, Default, Args, PartialEq, Eq)]
pub struct VoiceSetupCommand {
    /// 输出格式；省略时继承上层 setup 设置。
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
    /// 语音入口总开关；`--enable` 为开，`--enable false` 为关。
    #[arg(long, num_args = 0..=1, default_missing_value = "true")]
    pub enable: Option<bool>,
    /// 语音车道：cascade / realtime / auto。
    #[arg(long, value_enum)]
    pub lane: Option<VoiceLaneArg>,
    /// 渠道名称；给出渠道级参数时必须指定。
    #[arg(long)]
    pub channel: Option<String>,
    /// 新增渠道的协议；已知供应商自动推导。
    #[arg(long, value_enum)]
    pub protocol: Option<SetupProviderProtocol>,
    /// 新增渠道的 API 地址。
    #[arg(long)]
    pub base_url: Option<String>,
    /// 新增渠道的传输链路。
    #[arg(long, value_enum)]
    pub wire_transport: Option<SetupWireTransport>,
    /// 新增渠道的 WebSocket 地址。
    #[arg(long)]
    pub websocket_url: Option<String>,
    /// 新增渠道的 API key 环境变量名。
    #[arg(long)]
    pub api_key_env: Option<String>,
    /// 模型路由名称；给出模型级参数时必须指定。
    #[arg(long)]
    pub model: Option<String>,
    /// 上游模型标识。
    #[arg(long)]
    pub id: Option<String>,
    /// 语音模型角色：asr / tts / realtime。
    #[arg(long, value_enum)]
    pub role: Option<VoiceRoleArg>,
    /// 音色标识；tts / realtime 角色必填。
    #[arg(long)]
    pub speaker: Option<String>,
    /// 语速，范围 -50..=100。
    #[arg(long)]
    pub rate: Option<i32>,
    /// 轮次检测策略。
    #[arg(long, value_enum)]
    pub turn_detection: Option<VoiceTurnDetectionArg>,
    /// 重置整个 `[voice]` 段为默认值。
    #[arg(long)]
    pub reset: bool,
}

impl VoiceSetupCommand {
    /// 是否带有任何写入意图。
    #[must_use]
    pub fn has_mutation(&self) -> bool {
        self.reset
            || self.enable.is_some()
            || self.lane.is_some()
            || self.channel.is_some()
            || self.protocol.is_some()
            || self.base_url.is_some()
            || self.wire_transport.is_some()
            || self.websocket_url.is_some()
            || self.api_key_env.is_some()
            || self.model.is_some()
            || self.id.is_some()
            || self.role.is_some()
            || self.speaker.is_some()
            || self.rate.is_some()
            || self.turn_detection.is_some()
    }
}
