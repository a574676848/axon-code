//! 独立 `[voice]` 段：用户层语音渠道与模型配置。
//!
//! 公共字段同构复用 `ProviderConfig` / `ModelConfig` 的语义（渠道即 provider、
//! 音色即模型参数），但存储独立成段，避免与文本模型目录混用；仅 `role` /
//! `voice` / `speech_rate` / `turn_detection` 为本模块新增的语音专属语义。

use std::collections::BTreeSet;

use axon_domain::{AxonError, AxonResult};
use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_CHANNEL_PRIORITY, DEFAULT_CHANNEL_WEIGHT, PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL,
    PROVIDER_WIRE_TRANSPORT_WEBSOCKET,
};

/// 语音车道：级联。
pub const VOICE_LANE_CASCADE: &str = "cascade";
/// 语音车道：端到端 realtime。
pub const VOICE_LANE_REALTIME: &str = "realtime";
/// 语音车道：自动选择。
pub const VOICE_LANE_AUTO: &str = "auto";

/// 语音模型角色：识别。
pub const VOICE_ROLE_ASR: &str = "asr";
/// 语音模型角色：合成。
pub const VOICE_ROLE_TTS: &str = "tts";
/// 语音模型角色：端到端。
pub const VOICE_ROLE_REALTIME: &str = "realtime";

/// 轮次检测：服务端 VAD。
pub const VOICE_TURN_SERVER_VAD: &str = "server_vad";
/// 轮次检测：语义轮次。
pub const VOICE_TURN_SMART_TURN: &str = "smart_turn_v2";
/// 轮次检测：手动（PTT）。
pub const VOICE_TURN_MANUAL: &str = "manual";

/// 默认语音渠道协议。
pub const DEFAULT_VOICE_PROTOCOL: &str = PROVIDER_PROTOCOL_VOLCENGINE_ARK_MULTIMODAL;
/// 默认语音传输链路。
pub const DEFAULT_VOICE_WIRE_TRANSPORT: &str = PROVIDER_WIRE_TRANSPORT_WEBSOCKET;
/// 语速下限。
pub const VOICE_SPEECH_RATE_MIN: i32 = -50;
/// 语速上限。
pub const VOICE_SPEECH_RATE_MAX: i32 = 100;

/// 语音渠道：公共字段与 `ProviderConfig` 同构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceChannelConfig {
    /// 渠道名称，段内唯一，供模型 `channel` 引用。
    pub name: String,
    /// 是否参与语音渠道路由。
    pub enabled: bool,
    /// 协议类型，复用 provider 协议族。
    pub protocol: String,
    /// API 基址。
    pub base_url: String,
    /// API key 环境变量名。
    pub api_key_env: Option<String>,
    /// 直接配置的 API key，优先级高于 `api_key_env`。
    pub api_key: Option<String>,
    /// 底层传输链路：http 或 websocket。
    pub wire_transport: String,
    /// WebSocket URL，为空时由 `base_url` 推导。
    pub websocket_url: Option<String>,
    /// 渠道优先级，数值越大越优先。
    pub priority: i32,
    /// 同优先级渠道权重。
    pub weight: u32,
}

impl Default for VoiceChannelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            enabled: true,
            protocol: DEFAULT_VOICE_PROTOCOL.to_owned(),
            base_url: String::new(),
            api_key_env: None,
            api_key: None,
            wire_transport: DEFAULT_VOICE_WIRE_TRANSPORT.to_owned(),
            websocket_url: None,
            priority: DEFAULT_CHANNEL_PRIORITY,
            weight: DEFAULT_CHANNEL_WEIGHT,
        }
    }
}

/// 语音模型：公共字段与 `ModelConfig` 同构，另加语音专属字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceModelConfig {
    /// 模型路由名称，段内唯一。
    pub name: String,
    /// 引用的渠道名称，对应 `VoiceChannelConfig::name`。
    pub channel: String,
    /// 上游模型标识。
    pub id: String,
    /// 语音角色：asr / tts / realtime。
    pub role: String,
    /// 音色标识；tts / realtime 角色必填。
    pub voice: String,
    /// 语速，范围 -50..=100，0 为默认。
    pub speech_rate: i32,
    /// 轮次检测策略：server_vad / smart_turn_v2 / manual。
    pub turn_detection: String,
    /// 会话系统提示词，仅 realtime 角色生效；留空用内置语音版人格。
    pub instructions: String,
}

impl Default for VoiceModelConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            channel: String::new(),
            id: String::new(),
            role: VOICE_ROLE_ASR.to_owned(),
            voice: String::new(),
            speech_rate: 0,
            turn_detection: VOICE_TURN_SERVER_VAD.to_owned(),
            instructions: String::new(),
        }
    }
}

/// 用户层语音配置（独立 `[voice]` 段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// 语音总开关，对应 UI feature flag；默认关闭。
    pub enabled: bool,
    /// 语音车道：cascade / realtime / auto。
    pub channel_balancer: String,
    /// 语音渠道目录。
    pub channels: Vec<VoiceChannelConfig>,
    /// 语音模型目录。
    pub models: Vec<VoiceModelConfig>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            channel_balancer: VOICE_LANE_CASCADE.to_owned(),
            channels: Vec::new(),
            models: Vec::new(),
        }
    }
}

impl VoiceConfig {
    /// 语音入口是否启用。
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 当前语音车道标记。
    #[must_use]
    pub fn lane(&self) -> &str {
        &self.channel_balancer
    }

    /// 校验语音段安全不变量。
    ///
    /// # Errors
    ///
    /// 车道 / 角色 / 轮次检测非法、音色缺失、语速越界或渠道引用不存在时报错。
    pub fn validate(&self) -> AxonResult<()> {
        if !matches!(
            self.channel_balancer.as_str(),
            VOICE_LANE_CASCADE | VOICE_LANE_REALTIME | VOICE_LANE_AUTO
        ) {
            return Err(AxonError::config_invalid(
                "voice.channel_balancer 只能是 cascade / realtime / auto",
            ));
        }

        let mut names = BTreeSet::new();
        for channel in &self.channels {
            if channel.name.trim().is_empty() {
                return Err(AxonError::config_invalid("voice.channels[].name 不能为空"));
            }
            if !names.insert(channel.name.as_str()) {
                return Err(AxonError::config_invalid(format!(
                    "voice.channels[].name 重复：{}",
                    channel.name
                )));
            }
        }

        for model in &self.models {
            if model.name.trim().is_empty() {
                return Err(AxonError::config_invalid("voice.models[].name 不能为空"));
            }
            if !names.contains(model.channel.as_str()) {
                return Err(AxonError::config_invalid(format!(
                    "voice.models[{}].channel 未引用已配置渠道：{}",
                    model.name, model.channel
                )));
            }
            if !matches!(
                model.role.as_str(),
                VOICE_ROLE_ASR | VOICE_ROLE_TTS | VOICE_ROLE_REALTIME
            ) {
                return Err(AxonError::config_invalid(format!(
                    "voice.models[{}].role 只能是 asr / tts / realtime",
                    model.name
                )));
            }
            if matches!(model.role.as_str(), VOICE_ROLE_TTS | VOICE_ROLE_REALTIME)
                && model.voice.trim().is_empty()
            {
                return Err(AxonError::config_invalid(format!(
                    "voice.models[{}].role 为 tts / realtime 时 voice 必填",
                    model.name
                )));
            }
            if !(VOICE_SPEECH_RATE_MIN..=VOICE_SPEECH_RATE_MAX).contains(&model.speech_rate) {
                return Err(AxonError::config_invalid(format!(
                    "voice.models[{}].speech_rate 超出 -50..=100",
                    model.name
                )));
            }
            if !matches!(
                model.turn_detection.as_str(),
                VOICE_TURN_SERVER_VAD | VOICE_TURN_SMART_TURN | VOICE_TURN_MANUAL
            ) {
                return Err(AxonError::config_invalid(format!(
                    "voice.models[{}].turn_detection 只能是 server_vad / smart_turn_v2 / manual",
                    model.name
                )));
            }
        }
        Ok(())
    }
}

/// 供 `validators::validate_config` 调用的语音段校验入口。
pub(crate) fn validate_voice(voice: &VoiceConfig) -> AxonResult<()> {
    if !voice.enabled && voice.models.is_empty() && voice.channels.is_empty() {
        return Ok(());
    }
    voice.validate()
}

#[cfg(test)]
mod tests {
    use super::{
        VOICE_LANE_REALTIME, VOICE_ROLE_TTS, VoiceChannelConfig, VoiceConfig, VoiceModelConfig,
    };

    fn sample_config() -> VoiceConfig {
        VoiceConfig {
            enabled: true,
            channel_balancer: VOICE_LANE_REALTIME.to_owned(),
            channels: vec![VoiceChannelConfig {
                name: "volc-tts".to_owned(),
                ..VoiceChannelConfig::default()
            }],
            models: vec![VoiceModelConfig {
                name: "volc-tts-seed20".to_owned(),
                channel: "volc-tts".to_owned(),
                role: VOICE_ROLE_TTS.to_owned(),
                voice: "zh_female_shuangkuaisisi_uranus_bigtts".to_owned(),
                speech_rate: 20,
                ..VoiceModelConfig::default()
            }],
        }
    }

    #[test]
    fn default_voice_config_is_valid_and_disabled() {
        let config = VoiceConfig::default();
        assert!(!config.is_enabled());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn toml_roundtrip_preserves_all_voice_fields() {
        let config = sample_config();
        let text = toml::to_string(&config).expect("序列化应成功");
        let restored: VoiceConfig = toml::from_str(&text).expect("反序列化应成功");
        assert_eq!(config, restored);
    }

    #[test]
    fn sample_config_validates() {
        assert!(sample_config().validate().is_ok());
    }

    #[test]
    fn tts_model_requires_voice() {
        let mut config = sample_config();
        config.models[0].voice.clear();
        assert!(config.validate().is_err());
    }

    #[test]
    fn model_channel_must_exist() {
        let mut config = sample_config();
        config.models[0].channel = "missing".to_owned();
        assert!(config.validate().is_err());
    }

    #[test]
    fn speech_rate_bounds_are_enforced() {
        let mut config = sample_config();
        config.models[0].speech_rate = 101;
        assert!(config.validate().is_err());
        config.models[0].speech_rate = -51;
        assert!(config.validate().is_err());
    }
}
