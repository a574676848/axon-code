//! 独立 `[voice]` 段的读整段与写整段 IPC。
//!
//! 语音渠道与模型是结构化数组，扁平键值通道表达不了增删，因此这里提供专用
//! 命令：读时把整段投影成脱敏 DTO，写时整段替换。落盘复用 `catalog::edit_config`
//! 的一条链路（GET 权威全文 → 内存改 → PUT 全文 → 后台校验），不新开第二条。
//!
//! 安全边界：DTO 只带 `api_key_env` 变量名与 `api_keyConfigured` 状态，
//! `api_key` 明文绝不穿越 IPC；写回时按渠道名保留既有明文。

use axon_config::{ConfigLoader, VoiceChannelConfig, VoiceConfig, VoiceModelConfig};
use serde::{Deserialize, Serialize};

use crate::catalog::edit_config;
use crate::config_gateway::{self};
use crate::ipc::{ConfigReadResponse, ConfigWriteResponse};

/// 语音渠道 DTO：密钥只留变量名与是否已配置的状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceChannelDto {
    /// 渠道名称，段内唯一。
    pub name: String,
    /// 是否参与语音路由。
    pub enabled: bool,
    /// 协议类型，复用 provider 协议族。
    pub protocol: String,
    /// API 基址。
    pub base_url: String,
    /// API Key 环境变量名；不携带明文。
    pub api_key_env: Option<String>,
    /// 该渠道是否已配置密钥，仅用于展示。
    pub api_key_configured: bool,
    /// 底层传输链路。
    pub wire_transport: String,
    /// WebSocket 地址，为空时由基址推导。
    pub websocket_url: Option<String>,
    /// 渠道优先级。
    pub priority: i32,
    /// 同优先级权重。
    pub weight: u32,
}

/// 语音模型 DTO。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceModelDto {
    /// 模型路由名称，段内唯一。
    pub name: String,
    /// 引用的渠道名称。
    pub channel: String,
    /// 上游模型标识。
    pub id: String,
    /// 语音角色：asr / tts / realtime。
    pub role: String,
    /// 音色标识。
    pub voice: String,
    /// 语速。
    pub speech_rate: i32,
    /// 轮次检测策略。
    pub turn_detection: String,
    /// 模型优先级，数值越大越优先。
    pub priority: i32,
    /// 同优先级模型的权重。
    pub weight: u32,
}

/// 整段语音配置 DTO：读写共用；写回时 `api_keyConfigured` 由后端忽略并按名保留明文。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VoiceConfigDto {
    /// 语音渠道目录。
    pub channels: Vec<VoiceChannelDto>,
    /// 语音模型目录。
    pub models: Vec<VoiceModelDto>,
}

fn to_dto(config: &VoiceConfig) -> VoiceConfigDto {
    VoiceConfigDto {
        channels: config
            .channels
            .iter()
            .map(|channel| VoiceChannelDto {
                name: channel.name.clone(),
                enabled: channel.enabled,
                protocol: channel.protocol.clone(),
                base_url: channel.base_url.clone(),
                api_key_env: channel.api_key_env.clone(),
                api_key_configured: channel.api_key.is_some()
                    || channel
                        .api_key_env
                        .as_deref()
                        .is_some_and(|name| !name.trim().is_empty()),
                wire_transport: channel.wire_transport.clone(),
                websocket_url: channel.websocket_url.clone(),
                priority: channel.priority,
                weight: channel.weight,
            })
            .collect(),
        models: config
            .models
            .iter()
            .map(|model| VoiceModelDto {
                name: model.name.clone(),
                channel: model.channel.clone(),
                id: model.id.clone(),
                role: model.role.clone(),
                voice: model.voice.clone(),
                speech_rate: model.speech_rate,
                turn_detection: model.turn_detection.clone(),
                priority: model.priority,
                weight: model.weight,
            })
            .collect(),
    }
}

fn from_dto(
    input: VoiceConfigDto,
    keys: &std::collections::BTreeMap<String, Option<String>>,
) -> VoiceConfig {
    VoiceConfig {
        channels: input
            .channels
            .into_iter()
            .map(|channel| {
                let name = channel.name;
                VoiceChannelConfig {
                    api_key: keys.get(&name).cloned().flatten(),
                    name,
                    enabled: channel.enabled,
                    protocol: channel.protocol,
                    base_url: channel.base_url,
                    api_key_env: channel.api_key_env,
                    wire_transport: channel.wire_transport,
                    websocket_url: channel.websocket_url,
                    priority: channel.priority,
                    weight: channel.weight,
                }
            })
            .collect(),
        models: input
            .models
            .into_iter()
            .map(|model| VoiceModelConfig {
                name: model.name,
                channel: model.channel,
                id: model.id,
                role: model.role,
                voice: model.voice,
                speech_rate: model.speech_rate,
                turn_detection: model.turn_detection,
                priority: model.priority,
                weight: model.weight,
            })
            .collect(),
    }
}

/// Tauri IPC 命令：读取脱敏后的整段语音配置。
///
/// # Errors
///
/// 网关不可达、回执结构不符或后台配置无法解析。
#[tauri::command(async)]
pub fn voice_config_read() -> Result<VoiceConfigDto, String> {
    let receipt = config_gateway::get_json(config_gateway::CONFIG_PATH)?;
    let read: ConfigReadResponse = config_gateway::decode(receipt)?;
    let loaded = ConfigLoader::load_from_str(&read.toml_content)
        .map_err(|error| format!("后台配置无法解析：{error:?}"))?;
    Ok(to_dto(&loaded.config.voice))
}

/// Tauri IPC 命令：整段替换语音配置并落盘，密钥明文按渠道名保留。
///
/// # Errors
///
/// 语音配置校验未通过、网关不可达或后台服务版本过旧。
#[tauri::command(async)]
pub fn voice_config_apply(input: VoiceConfigDto) -> Result<ConfigWriteResponse, String> {
    edit_config(move |config| {
        let keys = config
            .voice
            .channels
            .iter()
            .map(|channel| (channel.name.clone(), channel.api_key.clone()))
            .collect();
        config.voice = from_dto(input, &keys);
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_roundtrip_never_carries_api_key_plaintext() {
        let mut config = VoiceConfig::default();
        config.channels.push(VoiceChannelConfig {
            name: "volc-tts".to_owned(),
            api_key: Some("secret-value".to_owned()),
            api_key_env: Some("VOLC_API_KEY".to_owned()),
            ..VoiceChannelConfig::default()
        });
        let dto = to_dto(&config);
        let json = serde_json::to_string(&dto).expect("DTO 应可序列化");
        assert!(!json.contains("secret-value"), "密钥明文不得穿越 IPC");
        assert!(dto.channels[0].api_key_configured);
        assert_eq!(dto.channels[0].api_key_env.as_deref(), Some("VOLC_API_KEY"));

        let mut keys = std::collections::BTreeMap::new();
        keys.insert("volc-tts".to_owned(), Some("secret-value".to_owned()));
        let restored = from_dto(dto, &keys);
        assert_eq!(
            restored.channels[0].api_key.as_deref(),
            Some("secret-value")
        );
        assert_eq!(restored, config);
    }

    #[test]
    fn rename_drops_the_old_channel_key() {
        let mut config = VoiceConfig::default();
        config.channels.push(VoiceChannelConfig {
            name: "old".to_owned(),
            api_key: Some("old-secret".to_owned()),
            ..VoiceChannelConfig::default()
        });
        let mut dto = to_dto(&config);
        dto.channels[0].name = "new".to_owned();
        let mut keys = std::collections::BTreeMap::new();
        keys.insert("old".to_owned(), Some("old-secret".to_owned()));
        let restored = from_dto(dto, &keys);
        assert!(
            restored.channels[0].api_key.is_none(),
            "改名不继承旧渠道密钥"
        );
    }
}
