//! 云端语音提供方端口：阿里百炼与火山引擎 ASR/TTS 的唯一接入点。
//!
//! 识别与合成由云端提供方承担，daemon 侧只保留这个端口：协议帧、桥状态机、
//! 回合编排与停播闭环都不依赖具体提供方。接入提供方时，只要在这里返回一个
//! 实现了 [`HostTransport`] 的传输，并给出预热要请求的族与模型标识，
//! daemon 其余模块不需要改动。
//!
//! 端到端 realtime 是另一条车道：它不需要预热的族与模型，而是一个 WS 连接
//! 直连百炼，配置在这里统一读取。两条车道由 [`lane`] 裁决，缺凭据时如实回落
//! 到级联并报「提供方不可用」，不假装就绪。

use super::gateway_voice_realtime_protocol::{SessionConfig, TurnDetection};
use super::gateway_voice_worker::BoxedTransport;

/// 一次会话的提供方通道：传输、预热请求的族与模型标识。
pub(super) struct ProviderSession {
    /// 与提供方之间的帧传输。
    pub(super) transport: BoxedTransport,
    /// 预热请求的模型族（`vad`/`asr`/`tts`/`cue`）。
    pub(super) family: &'static str,
    /// 预热请求的模型标识，由提供方配置给出。
    pub(super) model: String,
}

/// 打开一次会话的提供方通道。
///
/// # Errors
///
/// 返回对外错误码。当前没有配置任何级联云端提供方，恒为 `provider_unavailable`。
pub(super) fn open() -> Result<ProviderSession, &'static str> {
    Err("provider_unavailable")
}

/// 语音车道。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Lane {
    /// 级联：ASR → 文本回合 → TTS。尚未接入云端提供方。
    Cascade,
    /// 端到端 realtime：一条 WS 连接完成识别、生成与合成。
    Realtime,
}

/// realtime 车道的连接参数与会话配置。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RealtimeEndpoint {
    /// 完整 WS 地址，已带 `model` 查询参数。
    pub(super) url: String,
    /// `Authorization: Bearer` 的凭据。
    pub(super) api_key: String,
    /// 上报给运行态与 `ready` 帧的模型标识。
    pub(super) model: String,
    /// 会话配置。
    pub(super) session: SessionConfig,
}

/// 运行车道：realtime 配置齐全即走 realtime，否则如实回落到级联。
#[cfg_attr(test, allow(dead_code))]
pub(super) fn lane() -> Lane {
    if realtime_endpoint().is_ok() {
        Lane::Realtime
    } else {
        Lane::Cascade
    }
}

/// 读取 realtime 配置。
///
/// # Errors
///
/// 缺少 API key 时返回 `provider_unavailable`：宁可报不可用，也不拿空凭据去连。
pub(super) fn realtime_endpoint() -> Result<RealtimeEndpoint, &'static str> {
    let Some(api_key) = first_env(&[ENV_REALTIME_API_KEY, ENV_DASHSCOPE_API_KEY]) else {
        return Err("provider_unavailable");
    };
    let model = env_or(ENV_REALTIME_MODEL, DEFAULT_REALTIME_MODEL);
    let base = env_or(ENV_REALTIME_URL, DEFAULT_REALTIME_BASE_URL);
    Ok(RealtimeEndpoint {
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

/// realtime 车道的模型标识；不可用时为空。
#[cfg_attr(test, allow(dead_code))]
pub(super) fn realtime_model() -> Option<String> {
    realtime_endpoint().ok().map(|endpoint| endpoint.model)
}

/// 默认端点：百炼北京区 realtime 服务。
const DEFAULT_REALTIME_BASE_URL: &str =
    "wss://ws-twfnk5udhmcshkby.cn-beijing.maas.aliyuncs.com/api-ws/v1/realtime";
/// 默认模型：探针验证过的端到端实时模型。
const DEFAULT_REALTIME_MODEL: &str = "qwen-audio-3.1-realtime-flash";
/// 默认音色：与 TTS 探针一致，避免两条车道听起来像两个人。
const DEFAULT_REALTIME_VOICE: &str = "longanlingxin_v3.1";
/// 默认系统提示词：先守住「短」这条产品红线，M5 会接入真实人格。
const DEFAULT_REALTIME_INSTRUCTIONS: &str =
    "你是知微，先生的私人助理。回答简短口语化，一次不超过两句话。";

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
