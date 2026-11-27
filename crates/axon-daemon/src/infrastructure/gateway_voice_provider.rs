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

/// realtime 车道的协议实现。
///
/// 配置里 `channel.protocol` 的厂商名在这里映射，构造期一次性分派到具体实现，
/// 会话循环不感知厂商，也不在每帧做字符串匹配。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RealtimeProtocol {
    /// 阿里百炼 Qwen-Audio Realtime：JSON 事件 + 24 kHz 下行。
    #[default]
    Bailian,
    /// 火山方舟实时语音：二进制帧 + 原生采样率下行。
    Volc,
}

impl RealtimeProtocol {
    /// 按配置里的协议名构造；未知取值回落百炼（M3 已验证的默认车道）。
    #[must_use]
    pub fn from_code(code: &str) -> Self {
        match code {
            "volcengine_ark" | "volc" => Self::Volc,
            _ => Self::Bailian,
        }
    }
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
    /// 提供方协议；决定会话循环实例化哪个实现。
    pub protocol: RealtimeProtocol,
    /// 会话配置。
    pub session: SessionConfig,
}

impl RealtimeEndpoint {
    /// 用用户配置里的最小字段构造。
    ///
    /// 人格**不在这里定**：建会话时由 [`VoiceRuntime::persona_instructions`] 从
    /// `SOUL.md` 现算，这里只放内置兜底。语速留给服务端：`[voice]` 段的语速是
    /// 合成语义（-50..=100），与 realtime 的倍率语义（0.5..=2.0）不同，不能互相搬运。
    #[must_use]
    pub fn from_config(
        base_url: &str,
        api_key: String,
        model: String,
        voice: String,
        language: &str,
        turn_detection: &str,
        protocol: RealtimeProtocol,
    ) -> Self {
        let url = with_model_query(base_url, &model);
        Self {
            url,
            api_key,
            model,
            protocol,
            session: SessionConfig {
                voice,
                instructions: DEFAULT_REALTIME_INSTRUCTIONS.to_owned(),
                speech_rate: None,
                language: language.to_owned(),
                turn_detection: TurnDetection::from_code(turn_detection),
                max_history_turns: None,
            },
        }
    }

    /// 建会话前注入本次的语音人格；人格由 `SOUL.md` 抽茧而来，不走配置。
    #[must_use]
    pub fn with_instructions(mut self, instructions: String) -> Self {
        self.session.instructions = instructions;
        self
    }
}

/// 语音运行参数快照：一次装配、全会话只读。
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceRuntime {
    realtime: Option<RealtimeEndpoint>,
    tts: Option<VolcTtsConfig>,
    asr: Option<VolcAsrConfig>,
    /// `SOUL.md` 路径：语音人格的唯一来源；None 时退回内置默认人格。
    soul_path: Option<std::path::PathBuf>,
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
            soul_path: None,
        }
    }

    /// 用已解析的部件装配。
    #[must_use]
    pub fn new(
        realtime: Option<RealtimeEndpoint>,
        tts: Option<VolcTtsConfig>,
        asr: Option<VolcAsrConfig>,
    ) -> Self {
        Self {
            realtime,
            tts,
            asr,
            soul_path: None,
        }
    }

    /// 绑定 `SOUL.md`：语音人格从这份文件抽茧，改完下一次会话就生效。
    #[must_use]
    pub fn with_soul(mut self, soul_path: std::path::PathBuf) -> Self {
        self.soul_path = Some(soul_path);
        self
    }

    /// 本次会话的语音人格：SOUL 抽茧，任何环节失败都回落到内置默认。
    ///
    /// 每次建会话现算而不是装配时缓存：改 SOUL 不用重启，也没有第二份配置可漂移。
    #[must_use]
    pub fn persona_instructions(&self) -> String {
        let Some(path) = self.soul_path.as_deref() else {
            return DEFAULT_REALTIME_INSTRUCTIONS.to_owned();
        };
        std::fs::read_to_string(path)
            .ok()
            .and_then(|raw| axon_persona::SoulDocument::parse(&raw).ok())
            .map(|soul| axon_persona::render_voice_instructions(&soul.core))
            .filter(|text| !text.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_REALTIME_INSTRUCTIONS.to_owned())
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
        // 环境兜底路径没有 `[voice]` 段可读，固定走 M3 已验证的百炼车道。
        protocol: RealtimeProtocol::Bailian,
        url: with_model_query(&base, &model),
        api_key,
        model,
        session: SessionConfig {
            voice: env_or(ENV_REALTIME_VOICE, DEFAULT_REALTIME_VOICE),
            // 人格只有一份真源（SOUL）；env 不能再成为第三种写法。
            instructions: DEFAULT_REALTIME_INSTRUCTIONS.to_owned(),
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
/// 内置兜底人格：SOUL 缺失或解析失败时才会用到。
///
/// 正常路径是 `SOUL.md` 经 [`VoiceRuntime::persona_instructions`] 抽茧；
/// realtime 的 `instructions` 每轮随会话重发，整份 SOUL 会拖慢首包并稀释指令，
/// 所以抽茧只留人格骨架与语音红线。
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

    /// 真实 `SOUL.md` 抽茧：只在开发机手动执行，确认格式与口径真的对得上。
    #[test]
    #[ignore = "读真实 ~/.axon/SOUL.md，按需手动执行"]
    fn real_soul_renders_voice_instructions() {
        let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        else {
            return;
        };
        let soul_path = std::path::PathBuf::from(home).join(".axon").join("SOUL.md");
        if !soul_path.is_file() {
            return;
        }
        let runtime = VoiceRuntime::new(None, None, None).with_soul(soul_path);
        let text = runtime.persona_instructions();
        assert!(text.contains("知微"), "抽茧必须带身份名，实际：{text}");
        assert!(text.contains("语音红线"), "抽茧必须带语音红线");
        assert!(text.len() <= 1_300, "抽茧超出预算：{} 字节", text.len());
    }

    /// 没有 SOUL 时回落到内置兜底，不把会话打成空提示。
    #[test]
    fn missing_soul_falls_back_to_builtin_persona() {
        let runtime = VoiceRuntime::new(None, None, None);
        let text = runtime.persona_instructions();
        assert!(text.contains("不用 markdown"));
    }

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
