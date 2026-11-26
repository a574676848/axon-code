//! 云端语音提供方端口：阿里百炼与火山引擎 ASR/TTS 的唯一接入点。
//!
//! 识别与合成由云端提供方承担，daemon 侧只保留这个端口：协议帧、桥状态机、
//! 回合编排与停播闭环都不依赖具体提供方。接入提供方时，只要在这里返回一个
//! 实现了 [`HostTransport`] 的传输，并给出预热要请求的族与模型标识，
//! daemon 其余模块不需要改动。
//!
//! 运行参数由 [`VoiceRuntime`] 承载：用户层 `[voice]` 段经 CLI 映射后注入，
//! 未注入时按进程环境兜底。两条车道都由这一份快照裁决，运行期不再反复读
//! 环境变量，配置改动只在下次启动生效——行为可预期优先于热更新。

use super::gateway_voice_realtime_protocol::{SessionConfig, TurnDetection};
use super::gateway_voice_worker::BoxedTransport;
use super::voice_asr_volc::{
    DEFAULT_ASR_ENDPOINT, DEFAULT_ASR_LANGUAGE, DEFAULT_ASR_MODEL, DEFAULT_ASR_RESOURCE_ID,
    VolcAsrConfig, VolcAsrTransport,
};
use super::voice_tts_volc::{
    DEFAULT_TTS_ENDPOINT, DEFAULT_TTS_RESOURCE_ID, DEFAULT_TTS_SPEAKER, VolcTtsConfig,
};

/// 一次会话的提供方通道：传输、预热请求的族与模型标识。
pub(super) struct ProviderSession {
    /// 与提供方之间的帧传输。
    pub(super) transport: BoxedTransport,
    /// 预热请求的模型族（`vad`/`asr`/`tts`/`cue`）。
    pub(super) family: &'static str,
    /// 预热请求的模型标识，由提供方配置给出。
    pub(super) model: String,
}

/// 打开一次级联会话的提供方通道。
///
/// # Errors
///
/// 返回对外错误码。级联车道缺少识别渠道配置或握手失败时为 `provider_unavailable`。
pub(super) fn open(runtime: &VoiceRuntime) -> Result<ProviderSession, &'static str> {
    let config = runtime.asr().ok_or("provider_unavailable")?;
    let transport = VolcAsrTransport::open(config).map_err(|_| "provider_unavailable")?;
    Ok(ProviderSession {
        transport: Box::new(transport),
        family: "asr",
        model: config.model.clone(),
    })
}

/// 语音车道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// 级联：ASR → 文本回合 → TTS。尚未接入云端流式识别。
    Cascade,
    /// 端到端 realtime：一条 WS 连接完成识别、生成与合成。
    Realtime,
}

/// realtime 车道的连接参数与会话配置。
#[derive(Debug, Clone, PartialEq)]
pub struct RealtimeEndpoint {
    /// 完整 WS 地址，已带 `model` 查询参数。
    pub url: String,
    /// `Authorization: Bearer` 的凭据。
    pub api_key: String,
    /// 上报给运行态与 `ready` 帧的模型标识。
    pub model: String,
    /// 会话配置。
    pub session: SessionConfig,
}

impl RealtimeEndpoint {
    /// 用用户配置里的最小字段构造。
    ///
    /// 提示词按 `[voice]` 段的 `instructions` 走，留空回落语音版人格默认值；
    /// 语速留给服务端：`[voice]` 段的语速是合成语义（-50..=100），与 realtime
    /// 的倍率语义（0.5..=2.0）不同，不能互相搬运。
    #[must_use]
    pub fn from_config(
        base_url: &str,
        api_key: String,
        model: String,
        voice: String,
        language: &str,
        turn_detection: &str,
        instructions: &str,
    ) -> Self {
        let url = with_model_query(base_url, &model);
        let instructions = instructions.trim();
        Self {
            url,
            api_key,
            model,
            session: SessionConfig {
                voice,
                instructions: if instructions.is_empty() {
                    DEFAULT_REALTIME_INSTRUCTIONS.to_owned()
                } else {
                    instructions.to_owned()
                },
                speech_rate: None,
                language: language.to_owned(),
                turn_detection: TurnDetection::from_code(turn_detection),
                max_history_turns: None,
            },
        }
    }
}

/// 语音运行参数快照：一次装配、全会话只读。
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceRuntime {
    realtime: Option<RealtimeEndpoint>,
    tts: Option<VolcTtsConfig>,
    asr: Option<VolcAsrConfig>,
}

impl VoiceRuntime {
    /// 按进程环境兜底装配。
    ///
    /// 用户层 `[voice]` 段未注入时走这条路径：凭据齐全才认为车道可用，
    /// 缺凭据如实返回空，不拿空串去连。
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            realtime: realtime_from_env(),
            tts: tts_from_env(),
            asr: asr_from_env(),
        }
    }

    /// 用已解析的部件装配。
    #[must_use]
    pub fn new(
        realtime: Option<RealtimeEndpoint>,
        tts: Option<VolcTtsConfig>,
        asr: Option<VolcAsrConfig>,
    ) -> Self {
        Self { realtime, tts, asr }
    }

    /// realtime 车道参数；缺凭据时为 `None`。
    #[must_use]
    pub fn realtime(&self) -> Option<&RealtimeEndpoint> {
        self.realtime.as_ref()
    }

    /// 合成渠道参数；缺凭据时为 `None`。
    #[must_use]
    pub fn tts(&self) -> Option<&VolcTtsConfig> {
        self.tts.as_ref()
    }

    /// 识别渠道参数；缺凭据时为 `None`。
    #[must_use]
    pub fn asr(&self) -> Option<&VolcAsrConfig> {
        self.asr.as_ref()
    }

    /// 运行车道：realtime 配置齐全即走 realtime，否则如实回落到级联。
    #[must_use]
    pub fn lane(&self) -> Lane {
        if self.realtime.is_some() {
            Lane::Realtime
        } else {
            Lane::Cascade
        }
    }

    /// realtime 车道的模型标识；不可用时为空。
    #[must_use]
    pub fn realtime_model(&self) -> Option<String> {
        self.realtime
            .as_ref()
            .map(|endpoint| endpoint.model.clone())
    }
}

/// 读取 realtime 配置。
fn realtime_from_env() -> Option<RealtimeEndpoint> {
    let api_key = first_env(&[ENV_REALTIME_API_KEY, ENV_DASHSCOPE_API_KEY])?;
    let model = env_or(ENV_REALTIME_MODEL, DEFAULT_REALTIME_MODEL);
    let base = env_or(ENV_REALTIME_URL, DEFAULT_REALTIME_BASE_URL);
    Some(RealtimeEndpoint {
        url: with_model_query(&base, &model),
        api_key,
        model,
        session: SessionConfig {
            voice: env_or(ENV_REALTIME_VOICE, DEFAULT_REALTIME_VOICE),
            instructions: env_or(ENV_REALTIME_INSTRUCTIONS, DEFAULT_REALTIME_INSTRUCTIONS),
            speech_rate: env_number::<f32>(ENV_REALTIME_SPEECH_RATE)
                .filter(|rate| (0.5..=2.0).contains(rate)),
            language: env_or(ENV_REALTIME_LANGUAGE, "zh"),
            turn_detection: match env_or(ENV_REALTIME_TURN_DETECTION, "server_vad").as_str() {
                "manual" => TurnDetection::Manual,
                _ => TurnDetection::ServerVad {
                    threshold: env_number::<f32>(ENV_REALTIME_VAD_THRESHOLD)
                        .filter(|value| (-1.0..=1.0).contains(value))
                        .unwrap_or(super::gateway_voice_realtime_protocol::DEFAULT_VAD_THRESHOLD),
                    silence_ms: env_number::<u32>(ENV_REALTIME_SILENCE_MS)
                        .filter(|value| (200..=6_000).contains(value))
                        .unwrap_or(super::gateway_voice_realtime_protocol::DEFAULT_SILENCE_MS),
                },
            },
            max_history_turns: env_number::<u32>(ENV_REALTIME_MAX_HISTORY_TURNS)
                .filter(|value| (1..=50).contains(value)),
        },
    })
}

/// 读取合成渠道参数：语音专用变量优先，回退到方舟通用凭据。
fn tts_from_env() -> Option<VolcTtsConfig> {
    // 方舟的 Agent Plan 用同一把 ARK 凭据签语音，这里按「专用 → 通用」顺序取。
    let api_key = first_env(&[ENV_TTS_API_KEY, ENV_ARK_KEY, ENV_VOLC_API_KEY])?;
    Some(VolcTtsConfig {
        endpoint: env_or(ENV_TTS_URL, DEFAULT_TTS_ENDPOINT),
        api_key,
        resource_id: env_or(ENV_TTS_RESOURCE_ID, DEFAULT_TTS_RESOURCE_ID),
        speaker: env_or(ENV_TTS_SPEAKER, DEFAULT_TTS_SPEAKER),
        speech_rate: env_number::<i32>(ENV_TTS_SPEECH_RATE)
            .filter(|rate| (-50..=100).contains(rate))
            .unwrap_or(0),
    })
}

/// 读取识别渠道参数：识别专用变量优先，回退到方舟通用凭据。
///
/// 端点缺省为 Agent Plan 的流式识别通道；`nostream` 指「一次连接一次请求」，
/// 与上层「一句一收尾」的合同一致，不是禁用流式。
fn asr_from_env() -> Option<VolcAsrConfig> {
    let api_key = first_env(&[ENV_ASR_API_KEY, ENV_ARK_KEY, ENV_VOLC_API_KEY])?;
    Some(VolcAsrConfig::new(
        env_or(ENV_ASR_URL, DEFAULT_ASR_ENDPOINT),
        api_key,
        env_or(ENV_ASR_RESOURCE_ID, DEFAULT_ASR_RESOURCE_ID),
        env_or(ENV_ASR_MODEL, DEFAULT_ASR_MODEL),
        env_or(ENV_ASR_LANGUAGE, DEFAULT_ASR_LANGUAGE),
    ))
}

/// 默认端点：百炼北京区 realtime 服务。
const DEFAULT_REALTIME_BASE_URL: &str =
    "wss://ws-twfnk5udhmcshkby.cn-beijing.maas.aliyuncs.com/api-ws/v1/realtime";
/// 默认模型：探针验证过的端到端实时模型。
const DEFAULT_REALTIME_MODEL: &str = "qwen-audio-3.1-realtime-flash";
/// 默认音色：与 TTS 探针一致，避免两条车道听起来像两个人。
const DEFAULT_REALTIME_VOICE: &str = "longanlingxin_v3.1";
/// 默认系统提示词：语音版人格骨架。
///
/// realtime 的 `instructions` 每轮随会话重发，整份 SOUL 会拖慢首包并稀释
/// 指令，所以这里只留人格骨架与语音红线；完整人格由桌面文字侧承载。
const DEFAULT_REALTIME_INSTRUCTIONS: &str = "\
你是知微，先生的私人助理。\
性情知性干练、外冷内热：对技术只认规范与落地闭环，对生活直接给方案、不唠叨。\
说话短句、口语、先说结论，一次最多两句话；不用列表、不用 markdown、不复述先生的话。\
称呼对方为「先生」，自称「我」；不确定就直说不确定，不编造。";

/// 专用凭据变量名。
const ENV_REALTIME_API_KEY: &str = "AXON_VOICE_REALTIME_API_KEY";
/// 百炼通用凭据变量名，作为专用变量缺省时的回退。
const ENV_DASHSCOPE_API_KEY: &str = "DASHSCOPE_API_KEY";
/// 端点覆盖。
const ENV_REALTIME_URL: &str = "AXON_VOICE_REALTIME_URL";
/// 模型覆盖。
const ENV_REALTIME_MODEL: &str = "AXON_VOICE_REALTIME_MODEL";
/// 音色覆盖。
const ENV_REALTIME_VOICE: &str = "AXON_VOICE_REALTIME_VOICE";
/// 提示词覆盖。
const ENV_REALTIME_INSTRUCTIONS: &str = "AXON_VOICE_REALTIME_INSTRUCTIONS";
/// 语速覆盖。
const ENV_REALTIME_SPEECH_RATE: &str = "AXON_VOICE_REALTIME_SPEECH_RATE";
/// 转写语言覆盖。
const ENV_REALTIME_LANGUAGE: &str = "AXON_VOICE_REALTIME_LANGUAGE";
/// 历史轮数覆盖。
const ENV_REALTIME_MAX_HISTORY_TURNS: &str = "AXON_VOICE_REALTIME_MAX_HISTORY_TURNS";
/// VAD 静音时长覆盖。
const ENV_REALTIME_SILENCE_MS: &str = "AXON_VOICE_REALTIME_SILENCE_MS";
/// VAD 阈值覆盖。
const ENV_REALTIME_VAD_THRESHOLD: &str = "AXON_VOICE_REALTIME_VAD_THRESHOLD";
/// 轮次检测策略覆盖：`manual` 为按键说话，缺省走服务端 VAD。
const ENV_REALTIME_TURN_DETECTION: &str = "AXON_VOICE_REALTIME_TURN_DETECTION";

/// 合成专用凭据变量名。
const ENV_TTS_API_KEY: &str = "AXON_VOICE_TTS_API_KEY";
/// 方舟 Agent Plan 通用凭据：语音与时序模型共用同一把。
const ENV_ARK_KEY: &str = "ARK_KEY";
/// 火山引擎通用凭据。
const ENV_VOLC_API_KEY: &str = "VOLC_API_KEY";
/// 合成端点覆盖。
const ENV_TTS_URL: &str = "AXON_VOICE_TTS_URL";
/// 合成资源标识覆盖。
const ENV_TTS_RESOURCE_ID: &str = "AXON_VOICE_TTS_RESOURCE_ID";
/// 合成音色覆盖。
const ENV_TTS_SPEAKER: &str = "AXON_VOICE_TTS_SPEAKER";
/// 合成语速覆盖。
const ENV_TTS_SPEECH_RATE: &str = "AXON_VOICE_TTS_SPEECH_RATE";

/// 识别专用凭据变量名。
const ENV_ASR_API_KEY: &str = "AXON_VOICE_ASR_API_KEY";
/// 识别端点覆盖。
const ENV_ASR_URL: &str = "AXON_VOICE_ASR_URL";
/// 识别资源标识覆盖。
const ENV_ASR_RESOURCE_ID: &str = "AXON_VOICE_ASR_RESOURCE_ID";
/// 识别模型标识覆盖。
const ENV_ASR_MODEL: &str = "AXON_VOICE_ASR_MODEL";
/// 识别语言覆盖。
const ENV_ASR_LANGUAGE: &str = "AXON_VOICE_ASR_LANGUAGE";

fn env_or(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

fn first_env(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn env_number<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<T>().ok())
}

/// 把 `model` 拼进查询串；端点已经带了 `model` 参数时原样尊重。
fn with_model_query(base: &str, model: &str) -> String {
    if base.contains("model=") {
        return base.to_owned();
    }
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}model={model}")
}

#[cfg(test)]
mod tests {
    use super::super::voice_asr_volc::VolcAsrConfig;
    use super::super::voice_tts_volc::VolcTtsConfig;
    use super::{Lane, VoiceRuntime, with_model_query};

    fn tts_config() -> VolcTtsConfig {
        VolcTtsConfig {
            endpoint: "wss://example.invalid/tts".to_owned(),
            api_key: "key".to_owned(),
            resource_id: "seed-tts-2.0".to_owned(),
            speaker: "speaker".to_owned(),
            speech_rate: 0,
        }
    }

    #[test]
    fn runtime_without_realtime_falls_back_to_cascade() {
        let runtime = VoiceRuntime::new(None, Some(tts_config()), None);
        assert_eq!(runtime.lane(), Lane::Cascade);
        assert!(runtime.realtime().is_none());
        assert!(runtime.tts().is_some(), "合成与 realtime 相互独立");
        assert!(runtime.realtime_model().is_none());
    }

    #[test]
    fn runtime_without_tts_still_reports_its_lane() {
        let runtime = VoiceRuntime::new(None, None, None);
        assert_eq!(runtime.lane(), Lane::Cascade);
        assert!(runtime.tts().is_none(), "缺凭据必须如实为空");
    }

    #[test]
    fn asr_config_alone_keeps_the_cascade_lane() {
        let runtime = VoiceRuntime::new(
            None,
            None,
            Some(VolcAsrConfig::new(
                "wss://example.invalid/asr",
                "key",
                "volc.seedasr.sauc.duration",
                "",
                "zh",
            )),
        );
        assert_eq!(runtime.lane(), Lane::Cascade, "识别渠道本身不改变车道判定");
        let asr = runtime.asr().expect("识别渠道应当装配出来");
        assert_eq!(asr.model, super::DEFAULT_ASR_MODEL, "模型名缺省产品口径");
        assert!(runtime.realtime_model().is_none());
    }

    #[test]
    fn model_query_is_appended_once() {
        assert_eq!(
            with_model_query("wss://host/path", "m1"),
            "wss://host/path?model=m1"
        );
        assert_eq!(
            with_model_query("wss://host/path?x=1", "m1"),
            "wss://host/path?x=1&model=m1"
        );
        assert_eq!(
            with_model_query("wss://host/path?model=fixed", "m1"),
            "wss://host/path?model=fixed",
            "端点自带 model 时不再拼第二个"
        );
    }
}
