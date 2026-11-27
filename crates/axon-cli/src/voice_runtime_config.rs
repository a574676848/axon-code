//! 用户层 `[voice]` 段 → daemon 语音运行参数的映射。
//!
//! 只做字段搬运与凭据解析，不做协议决策：渠道协议、端点与资源标识怎么填，
//! 由 `[voice]` 段决定；daemon 收到什么就用什么。缺凭据的能力一律留空，
//! 由 daemon 如实报 `provider_unavailable`，这里不编造默认值、不猜厂商。

use axon_config::voice::{
    VOICE_ROLE_ASR, VOICE_ROLE_REALTIME, VOICE_ROLE_TTS, VoiceChannelConfig, VoiceConfig,
    VoiceModelConfig,
};
use axon_daemon::{RealtimeEndpoint, VoiceRuntime, VolcAsrConfig, VolcTtsConfig};
use std::path::Path;

/// 默认转写语言：与协议缺省一致。
const DEFAULT_LANGUAGE: &str = "zh";

/// 把 `[voice]` 段映射成运行参数。
///
/// `axon_root` 只用于定位 `SOUL.md`：语音人格从那份文件抽茧，`[voice]` 段
/// 不承载人格文本，避免出现第二份可漂移的真源。
#[must_use]
pub fn voice_runtime_from_config(voice: &VoiceConfig, axon_root: &Path) -> VoiceRuntime {
    VoiceRuntime::new(
        realtime_endpoint(voice),
        tts_config(voice),
        asr_config(voice),
    )
    .with_soul(axon_root.join("SOUL.md"))
}

/// 找出某个角色的模型定义。
fn model_of<'a>(voice: &'a VoiceConfig, role: &str) -> Option<&'a VoiceModelConfig> {
    voice.models.iter().find(|model| model.role == role)
}

/// 找出模型引用的渠道。
fn channel_of<'a>(
    voice: &'a VoiceConfig,
    model: &VoiceModelConfig,
) -> Option<&'a VoiceChannelConfig> {
    voice
        .channels
        .iter()
        .find(|channel| channel.name == model.channel)
}

/// 解析渠道凭据：直填优先，其次按环境变量名取，最后读凭据库。
fn credential(channel: &VoiceChannelConfig) -> Option<String> {
    channel.api_key.clone().or_else(|| {
        channel
            .api_key_env
            .as_deref()
            .and_then(|name| {
                axon_config::provider_credential(name).or_else(|| std::env::var(name).ok())
            })
            .filter(|value| !value.trim().is_empty())
    })
}

/// 渠道的完整 WS 地址：显式 `websocket_url` 优先，否则用 `base_url`。
fn endpoint_of(channel: &VoiceChannelConfig) -> Option<String> {
    let base = channel
        .websocket_url
        .clone()
        .unwrap_or_else(|| channel.base_url.clone());
    if base.trim().is_empty() {
        return None;
    }
    Some(base)
}

/// 端到端车道：`realtime` 角色。
fn realtime_endpoint(voice: &VoiceConfig) -> Option<RealtimeEndpoint> {
    let model = model_of(voice, VOICE_ROLE_REALTIME)?;
    let channel = channel_of(voice, model)?;
    let api_key = credential(channel)?;
    let base = endpoint_of(channel)?;
    Some(RealtimeEndpoint::from_config(
        &base,
        api_key,
        model.id.clone(),
        model.voice.clone(),
        DEFAULT_LANGUAGE,
        &model.turn_detection,
    ))
}

/// 合成渠道：`tts` 角色。
///
/// `id` 承载上游资源标识（方舟的 `seed-tts-2.0`），模型名只出现在文档口径里，
/// 协议不上送；`voice` 是音色标识，`speech_rate` 是合成语速。
fn tts_config(voice: &VoiceConfig) -> Option<VolcTtsConfig> {
    let model = model_of(voice, VOICE_ROLE_TTS)?;
    let channel = channel_of(voice, model)?;
    let api_key = credential(channel)?;
    let endpoint = endpoint_of(channel)?;
    Some(VolcTtsConfig {
        endpoint,
        api_key,
        resource_id: model.id.clone(),
        speaker: model.voice.clone(),
        speech_rate: model.speech_rate,
    })
}

/// 识别渠道：`asr` 角色。
///
/// `id` 承载上游资源标识（方舟的 `volc.seedasr.sauc.duration`）；协议上送的
/// 模型名由实现内置，内部路由名不冒充模型编号。
fn asr_config(voice: &VoiceConfig) -> Option<VolcAsrConfig> {
    let model = model_of(voice, VOICE_ROLE_ASR)?;
    let channel = channel_of(voice, model)?;
    let api_key = credential(channel)?;
    let endpoint = endpoint_of(channel)?;
    Some(VolcAsrConfig::new(
        endpoint,
        api_key,
        model.id.clone(),
        "",
        DEFAULT_LANGUAGE,
    ))
}

#[cfg(test)]
mod tests {
    use axon_config::voice::{
        VOICE_LANE_REALTIME, VOICE_ROLE_ASR, VOICE_ROLE_REALTIME, VOICE_ROLE_TTS,
        VoiceChannelConfig, VoiceConfig, VoiceModelConfig,
    };
    use axon_daemon::VoiceRuntime;

    /// 测试用包装：提示词文件按当前目录解析，用例只关心直填、读盘与缺文件三条路径。
    fn voice_runtime_from_config(voice: &VoiceConfig) -> VoiceRuntime {
        super::voice_runtime_from_config(voice, std::path::Path::new("."))
    }

    fn channel(name: &str) -> VoiceChannelConfig {
        VoiceChannelConfig {
            name: name.to_owned(),
            api_key: Some("key".to_owned()),
            base_url: "wss://example.invalid/voice".to_owned(),
            ..VoiceChannelConfig::default()
        }
    }

    #[test]
    fn empty_config_yields_no_capability() {
        let runtime = voice_runtime_from_config(&VoiceConfig::default());
        assert!(runtime.realtime().is_none());
        assert!(runtime.tts().is_none());
        assert!(runtime.asr().is_none());
    }

    #[test]
    fn tts_role_maps_into合成渠道() {
        let voice = VoiceConfig {
            channel_balancer: VOICE_LANE_REALTIME.to_owned(),
            channels: vec![channel("volc-tts")],
            models: vec![VoiceModelConfig {
                name: "volc-tts-seed20".to_owned(),
                channel: "volc-tts".to_owned(),
                id: "seed-tts-2.0".to_owned(),
                role: VOICE_ROLE_TTS.to_owned(),
                voice: "zh_female_shuangkuaisisi_uranus_bigtts".to_owned(),
                speech_rate: 20,
                ..VoiceModelConfig::default()
            }],
            ..VoiceConfig::default()
        };
        let runtime = voice_runtime_from_config(&voice);
        let tts = runtime.tts().expect("合成渠道必须装配出来");
        assert_eq!(tts.resource_id, "seed-tts-2.0");
        assert_eq!(tts.speech_rate, 20);
        assert_eq!(tts.endpoint, "wss://example.invalid/voice");
        assert!(
            runtime.realtime().is_none(),
            "没配 realtime 角色就不该有车道"
        );
    }

    #[test]
    fn realtime_role_maps_into车道参数() {
        let voice = VoiceConfig {
            channel_balancer: VOICE_LANE_REALTIME.to_owned(),
            channels: vec![channel("bailian")],
            models: vec![VoiceModelConfig {
                name: "qwen-realtime".to_owned(),
                channel: "bailian".to_owned(),
                id: "qwen-audio-3.1-realtime-flash".to_owned(),
                role: VOICE_ROLE_REALTIME.to_owned(),
                voice: "longanlingxin_v3.1".to_owned(),
                ..VoiceModelConfig::default()
            }],
            ..VoiceConfig::default()
        };
        let runtime = voice_runtime_from_config(&voice);
        let endpoint = runtime.realtime().expect("realtime 车道必须装配出来");
        assert!(endpoint.url.contains("model=qwen-audio-3.1-realtime-flash"));
        assert_eq!(endpoint.session.voice, "longanlingxin_v3.1");
        assert!(runtime.tts().is_none());
    }

    #[test]
    fn missing_credential_leaves_the_capability_empty() {
        let mut plain = channel("no-key");
        plain.api_key = None;
        let voice = VoiceConfig {
            channels: vec![plain],
            models: vec![VoiceModelConfig {
                name: "volc-tts".to_owned(),
                channel: "no-key".to_owned(),
                id: "seed-tts-2.0".to_owned(),
                role: VOICE_ROLE_TTS.to_owned(),
                voice: "speaker".to_owned(),
                ..VoiceModelConfig::default()
            }],
            ..VoiceConfig::default()
        };
        assert!(
            voice_runtime_from_config(&voice).tts().is_none(),
            "没有凭据就不能装配，交给 daemon 报不可用"
        );
    }

    #[test]
    fn asr_role_maps_into识别渠道() {
        let voice = VoiceConfig {
            channel_balancer: VOICE_LANE_REALTIME.to_owned(),
            channels: vec![channel("volc-asr")],
            models: vec![VoiceModelConfig {
                name: "volc-asr-seed20".to_owned(),
                channel: "volc-asr".to_owned(),
                id: "volc.seedasr.sauc.duration".to_owned(),
                role: VOICE_ROLE_ASR.to_owned(),
                ..VoiceModelConfig::default()
            }],
            ..VoiceConfig::default()
        };
        let runtime = voice_runtime_from_config(&voice);
        let asr = runtime.asr().expect("识别渠道必须装配出来");
        assert_eq!(asr.resource_id, "volc.seedasr.sauc.duration");
        assert_eq!(asr.endpoint, "wss://example.invalid/voice");
        assert_eq!(asr.language, "zh");
        assert!(runtime.realtime().is_none(), "asr 角色不等于 realtime 车道");
    }
}
