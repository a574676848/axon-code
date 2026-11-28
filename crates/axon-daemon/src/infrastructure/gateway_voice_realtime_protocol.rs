//! 百炼 realtime 语音协议：客户端事件编码、服务端事件解码、转写累积与采样率换算。
//!
//! 事实来自 M0-R 探针（48 个事件、六项检查全绿）与官方文档：
//! - 上行只接受 16 kHz / 16 bit / 单声道 PCM，下行固定 24 kHz PCM；
//! - `...input_audio_transcription.delta` 的 `text` 是已确定前缀、`stash` 是未确定尾部，
//!   两者都是累积值，逐条拼接必然重复，只能每次整体覆盖；
//! - 最终文本以 `.completed.transcript` 为准，它在 `response.done` 之后才到；
//! - 服务端不按实时节奏推音频，节流是客户端的事。
//!
//! 本模块只做纯计算与协议编解码，不碰 IO，因此协议口径全部可以用单测钉住。
//! 会话循环通过 [`RealtimeProvider`] 使用本实现，不直接依赖任何百炼细节。

use base64::Engine as _;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderName};
use tokio_tungstenite::tungstenite::protocol::Message;

use super::gateway_voice_realtime_provider::{RealtimeControl, RealtimeEvent, RealtimeProvider};

/// 单个上行分片：20 ms @ 16 kHz 单声道 s16le。
pub(super) const APPEND_CHUNK_BYTES: usize = 640;
/// 单个下行分片：20 ms @ 16 kHz 单声道 s16le。
pub(super) const PLAYBACK_CHUNK_BYTES: usize = 640;
/// 转写语言的缺省值。
pub(super) const DEFAULT_TRANSCRIPTION_LANGUAGE: &str = "zh";
/// 轮次检测的缺省阈值。
pub(super) const DEFAULT_VAD_THRESHOLD: f32 = 0.5;
/// 轮次检测的缺省静音时长。
pub(super) const DEFAULT_SILENCE_MS: u32 = 800;
/// 百炼的额外请求头：关闭数据巡检，端到端语音不上送内容审核。
const DATA_INSPECTION_HEADER: HeaderName = HeaderName::from_static("x-dashscope-datainspection");

/// 轮次检测策略。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TurnDetection {
    /// 服务端 VAD 自动断句。
    ServerVad {
        /// 语音起始阈值，取值 -1..=1。
        threshold: f32,
        /// 判定说完所需的静音时长。
        silence_ms: u32,
    },
    /// 手动提交：`turn_detection` 为 `null`，等价于按键说话。
    Manual,
}

impl Default for TurnDetection {
    fn default() -> Self {
        Self::ServerVad {
            threshold: DEFAULT_VAD_THRESHOLD,
            silence_ms: DEFAULT_SILENCE_MS,
        }
    }
}

impl TurnDetection {
    /// 按配置码构造：`manual` 为按键说话，其余一律走服务端 VAD 的缺省阈值。
    ///
    /// 配置层只懂「策略名」，阈值与静音时长属于协议细节，在这里收口。
    #[must_use]
    pub fn from_code(code: &str) -> Self {
        match code {
            "manual" => Self::Manual,
            _ => Self::default(),
        }
    }
}

/// 一次 realtime 会话的配置；字段与 `session.update` 一一对应。
#[derive(Debug, Clone, PartialEq)]
pub struct SessionConfig {
    /// 音色标识；协议规定只有会话首个 `session.update` 生效。
    pub voice: String,
    /// 系统提示词。
    pub instructions: String,
    /// 语速，协议范围 0.5..=2.0；`None` 表示按服务端默认。
    pub speech_rate: Option<f32>,
    /// 输入转写语言。
    pub language: String,
    /// 轮次检测策略。
    pub turn_detection: TurnDetection,
    /// 会话内保留的历史轮数，协议范围 1..=50；`None` 表示按服务端默认。
    pub max_history_turns: Option<u32>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            voice: String::new(),
            instructions: String::new(),
            speech_rate: None,
            language: DEFAULT_TRANSCRIPTION_LANGUAGE.to_owned(),
            turn_detection: TurnDetection::default(),
            max_history_turns: None,
        }
    }
}

impl SessionConfig {
    /// 构造 `session.update` 事件。
    ///
    /// 只写探针验证过的字段：协议的 `input_audio_format` / `output_audio_format`
    /// 只有单一合法取值，留给服务端默认，少写少错。
    pub(super) fn session_update_json(&self) -> String {
        let mut session = serde_json::json!({
            "modalities": ["text", "audio"],
            "voice": self.voice,
            "instructions": self.instructions,
            "input_audio_transcription": { "language": self.language },
            "turn_detection": self.turn_detection_json(),
        });
        let object = session.as_object_mut().expect("session 必然是 JSON 对象");
        if let Some(rate) = self.speech_rate {
            object.insert("speech_rate".to_owned(), serde_json::json!(rate));
        }
        if let Some(turns) = self.max_history_turns {
            object.insert("max_history_turns".to_owned(), serde_json::json!(turns));
        }
        serde_json::json!({ "type": "session.update", "session": session }).to_string()
    }

    fn turn_detection_json(&self) -> serde_json::Value {
        match self.turn_detection {
            TurnDetection::ServerVad {
                threshold,
                silence_ms,
            } => serde_json::json!({
                "type": "server_vad",
                "threshold": threshold,
                "silence_duration_ms": silence_ms,
            }),
            TurnDetection::Manual => serde_json::Value::Null,
        }
    }
}

/// 构造 `input_audio_buffer.append` 事件。
///
/// 手拼而不是走 `serde_json`：字段名与结构是编译期常量，序列化器在这里只增加
/// 一次中间对象分配；base64 也只编一次，直接写进预分配容量的字符串。
pub(super) fn audio_append_json(pcm: &[u8]) -> String {
    const PREFIX: &str = r#"{"type":"input_audio_buffer.append","audio":""#;
    const SUFFIX: &str = r#""}"#;
    // base64 输出长度：每 3 字节 4 字符，向上取整。
    let encoded_len = pcm.len().div_ceil(3) * 4;
    let mut out = String::with_capacity(PREFIX.len() + encoded_len + SUFFIX.len());
    out.push_str(PREFIX);
    base64::engine::general_purpose::STANDARD.encode_string(pcm, &mut out);
    out.push_str(SUFFIX);
    out
}

/// 构造 `input_audio_buffer.commit` 事件；服务端 VAD 下会被忽略。
pub(super) const COMMIT_JSON: &str = r#"{"type":"input_audio_buffer.commit"}"#;
/// 构造 `input_audio_buffer.clear` 事件。
pub(super) const CLEAR_JSON: &str = r#"{"type":"input_audio_buffer.clear"}"#;
/// 构造 `response.cancel` 事件。
pub(super) const CANCEL_JSON: &str = r#"{"type":"response.cancel"}"#;
/// 构造 `session.finish` 事件，正常收线用。
pub(super) const FINISH_JSON: &str = r#"{"type":"session.finish"}"#;

/// 百炼 realtime 提供方：JSON 事件 + Bearer 凭据 + 24 kHz 下行。
pub(super) struct BailianProvider {
    url: String,
    api_key: String,
    session: SessionConfig,
    resampler: Resampler24kTo16k,
}

impl BailianProvider {
    /// 用连接参数与会话配置构造。
    #[must_use]
    pub(super) fn new(url: String, api_key: String, session: SessionConfig) -> Self {
        Self {
            url,
            api_key,
            session,
            resampler: Resampler24kTo16k::new(),
        }
    }
}

impl RealtimeProvider for BailianProvider {
    fn handshake_request(&self) -> Result<Request, String> {
        let mut request = self
            .url
            .as_str()
            .into_client_request()
            .map_err(|_| "provider_unavailable".to_owned())?;
        let headers = request.headers_mut();
        let bearer = format!("Bearer {}", self.api_key);
        let value =
            HeaderValue::from_str(&bearer).map_err(|_| "provider_unavailable".to_owned())?;
        headers.insert(AUTHORIZATION, value);
        headers.insert(DATA_INSPECTION_HEADER, HeaderValue::from_static("disable"));
        Ok(request)
    }

    fn session_update(&self) -> String {
        self.session.session_update_json()
    }

    fn encode_audio(&self, pcm: &[u8], out: &mut Vec<Message>) {
        for chunk in pcm.chunks(APPEND_CHUNK_BYTES) {
            out.push(Message::Text(audio_append_json(chunk).into()));
        }
    }

    fn encode_control(&self, control: RealtimeControl) -> Message {
        let payload = match control {
            RealtimeControl::Commit => COMMIT_JSON,
            RealtimeControl::Clear => CLEAR_JSON,
            RealtimeControl::Cancel => CANCEL_JSON,
            RealtimeControl::Finish => FINISH_JSON,
        };
        Message::Text(payload.into())
    }

    fn decode_event(&self, text: &str) -> RealtimeEvent {
        parse_server_event(text)
    }

    fn resample_downlink(&mut self, pcm: &[u8]) -> Vec<u8> {
        self.resampler.push(pcm)
    }
}

/// 解析一条服务端事件；非法 JSON 视为 `Ignored`，不让脏帧打断会话。
pub(super) fn parse_server_event(text: &str) -> RealtimeEvent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return RealtimeEvent::Ignored;
    };
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return RealtimeEvent::Ignored;
    };
    match kind {
        "session.created" => RealtimeEvent::SessionCreated,
        "session.updated" => RealtimeEvent::SessionUpdated,
        "input_audio_buffer.speech_started" => RealtimeEvent::SpeechStarted,
        "input_audio_buffer.speech_stopped" => RealtimeEvent::SpeechStopped,
        "conversation.item.input_audio_transcription.delta" => {
            RealtimeEvent::InputTranscriptDelta {
                text: string_field(&value, "text"),
                stash: string_field(&value, "stash"),
            }
        }
        "conversation.item.input_audio_transcription.completed" => {
            RealtimeEvent::InputTranscriptCompleted {
                transcript: string_field(&value, "transcript"),
            }
        }
        "response.created" => RealtimeEvent::ResponseCreated,
        "response.audio_transcript.delta" | "response.text.delta" => {
            RealtimeEvent::OutputTranscriptDelta {
                text: string_field(&value, "delta"),
            }
        }
        "response.audio_transcript.done" => RealtimeEvent::OutputTranscriptDone {
            transcript: string_field(&value, "transcript"),
        },
        "response.audio.delta" => {
            let encoded = string_field(&value, "delta");
            match base64::engine::general_purpose::STANDARD.decode(encoded) {
                Ok(pcm) => RealtimeEvent::AudioDelta { pcm },
                Err(_) => RealtimeEvent::Ignored,
            }
        }
        "response.done" => RealtimeEvent::ResponseDone,
        "error" => RealtimeEvent::Error {
            code: error_code(&value),
            message: error_message(&value),
        },
        _ => RealtimeEvent::Ignored,
    }
}

fn string_field(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// 错误码优先取 `error.code`，其次顶层 `code`，都没有时给稳定兜底。
fn error_code(value: &serde_json::Value) -> String {
    for path in [("error", "code"), ("", "code")] {
        let node = if path.0.is_empty() {
            value
        } else {
            match value.get(path.0) {
                Some(node) => node,
                None => continue,
            }
        };
        if let Some(code) = node.get(path.1).and_then(serde_json::Value::as_str) {
            return code.to_owned();
        }
    }
    "provider_error".to_owned()
}

/// 错误说明：`error.message` 优先，其次顶层 `message`。
fn error_message(value: &serde_json::Value) -> String {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| value.get("message").and_then(serde_json::Value::as_str))
        .unwrap_or("语音提供方返回错误")
        .to_owned()
}

/// 合并累积型转写：`text` 是已确定前缀，`stash` 是未确定尾部。
///
/// 两者都是累积值而不是增量，调用方每次都必须整体覆盖，不能在旧文本上追加。
pub(super) fn merge_transcript(text: &str, stash: &str) -> String {
    if stash.is_empty() {
        return text.to_owned();
    }
    let mut merged = String::with_capacity(text.len() + stash.len());
    merged.push_str(text);
    merged.push_str(stash);
    merged
}

/// 24 kHz → 16 kHz 的流式重采样：每 3 个输入采样产出 2 个输出采样。
///
/// 第一个输出取组首采样，第二个取组内第 1、2 个采样的中点，
/// 相当于 3:2 抽取加半步线性插值。跨包余量留在结构里，分片边界不会产生爆音。
#[derive(Debug, Default)]
pub(super) struct Resampler24kTo16k {
    /// 上一包里没凑满一组的采样。
    carry_samples: Vec<i16>,
    /// 上一包末尾落单的字节，等待与本包首字节拼成一个采样。
    carry_byte: Option<u8>,
}

impl Resampler24kTo16k {
    /// 新建。
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// 送入 24 kHz s16le 字节流，返回 16 kHz s16le 字节流。
    pub(super) fn push(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut samples: Vec<i16> = Vec::with_capacity(self.carry_samples.len() + bytes.len() / 2);
        samples.extend_from_slice(&self.carry_samples);
        self.carry_samples.clear();

        let mut cursor = 0;
        if let Some(pending) = self.carry_byte.take() {
            let Some(first) = bytes.first() else {
                self.carry_byte = Some(pending);
                return Vec::new();
            };
            samples.push(i16::from_le_bytes([pending, *first]));
            cursor = 1;
        }
        let rest = &bytes[cursor..];
        let (pairs, remainder) = rest.as_chunks::<2>();
        for pair in pairs {
            samples.push(i16::from_le_bytes(*pair));
        }
        self.carry_byte = remainder.first().copied();

        let groups = samples.len() / 3;
        let mut out = Vec::with_capacity(groups * 4);
        for group in 0..groups {
            let base = group * 3;
            let head = samples[base];
            let middle = (i32::from(samples[base + 1]) + i32::from(samples[base + 2])) / 2;
            out.extend_from_slice(&head.to_le_bytes());
            out.extend_from_slice(&(middle as i16).to_le_bytes());
        }
        self.carry_samples.extend_from_slice(&samples[groups * 3..]);
        out
    }
}

/// 把 16 kHz PCM 切成等长播放分片；末尾不足一片也照常返回。
pub(super) fn playback_chunks(pcm: &[u8]) -> impl Iterator<Item = &[u8]> {
    pcm.chunks(PLAYBACK_CHUNK_BYTES)
}

#[cfg(test)]
#[path = "gateway_voice_realtime_protocol_tests.rs"]
mod tests;
