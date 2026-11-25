//! 百炼 realtime 语音协议：客户端事件编码、服务端事件解码、转写累积与采样率换算。
//!
//! 事实来自 M0-R 探针（48 个事件、六项检查全绿）与官方文档：
//! - 上行只接受 16 kHz / 16 bit / 单声道 PCM，下行固定 24 kHz PCM；
//! - `...input_audio_transcription.delta` 的 `text` 是已确定前缀、`stash` 是未确定尾部，
//!   两者都是累积值，逐条拼接必然重复，只能每次整体覆盖；
//! - 最终文本以 `.completed.transcript` 为准，它在 `response.done` 之后才到；
//! - 服务端不按实时节奏推音频，节流是客户端的事。
//!
//! 本模块只做纯计算，不碰 IO，因此协议口径全部可以用单测钉住。

use base64::Engine as _;

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

/// 轮次检测策略。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum TurnDetection {
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

/// 一次 realtime 会话的配置；字段与 `session.update` 一一对应。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SessionConfig {
    /// 音色标识；协议规定只有会话首个 `session.update` 生效。
    pub(super) voice: String,
    /// 系统提示词。
    pub(super) instructions: String,
    /// 语速，协议范围 0.5..=2.0；`None` 表示按服务端默认。
    pub(super) speech_rate: Option<f32>,
    /// 输入转写语言。
    pub(super) language: String,
    /// 轮次检测策略。
    pub(super) turn_detection: TurnDetection,
    /// 会话内保留的历史轮数，协议范围 1..=50；`None` 表示按服务端默认。
    pub(super) max_history_turns: Option<u32>,
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
pub(super) fn audio_append_json(pcm: &[u8]) -> String {
    let audio = base64::engine::general_purpose::STANDARD.encode(pcm);
    serde_json::json!({
        "type": "input_audio_buffer.append",
        "audio": audio,
    })
    .to_string()
}

/// 构造 `input_audio_buffer.commit` 事件；服务端 VAD 下会被忽略。
pub(super) const COMMIT_JSON: &str = r#"{"type":"input_audio_buffer.commit"}"#;
/// 构造 `input_audio_buffer.clear` 事件。
pub(super) const CLEAR_JSON: &str = r#"{"type":"input_audio_buffer.clear"}"#;
/// 构造 `response.cancel` 事件。
pub(super) const CANCEL_JSON: &str = r#"{"type":"response.cancel"}"#;
/// 构造 `session.finish` 事件，正常收线用。
pub(super) const FINISH_JSON: &str = r#"{"type":"session.finish"}"#;

/// 服务端事件；只保留会话循环真正消费的形状。
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ServerEvent {
    /// 连接建立，携带会话标识。
    SessionCreated,
    /// 配置生效；到这一刻才能开始送音频。
    SessionUpdated,
    /// 服务端 VAD 判定语音开始。
    SpeechStarted,
    /// 服务端 VAD 判定语音结束。
    SpeechStopped,
    /// 输入转写增量：`text` 是已确定前缀，`stash` 是未确定尾部。
    InputTranscriptDelta {
        /// 已确定前缀。
        text: String,
        /// 未确定尾部。
        stash: String,
    },
    /// 输入转写定稿，权威结果。
    InputTranscriptCompleted {
        /// 定稿文本。
        transcript: String,
    },
    /// 一轮回复开始；新一轮播放从这里分配轮次。
    ResponseCreated,
    /// 助手文字增量。
    OutputTranscriptDelta {
        /// 增量文本。
        text: String,
    },
    /// 助手文字定稿。
    OutputTranscriptDone {
        /// 定稿文本；服务端没给时为空。
        transcript: String,
    },
    /// 助手音频分片，已从 base64 解码，24 kHz s16le。
    AudioDelta {
        /// 解码后的 PCM。
        pcm: Vec<u8>,
    },
    /// 一轮回复结束。
    ResponseDone,
    /// 服务端错误。
    Error {
        /// 稳定错误码。
        code: String,
        /// 服务端给出的说明。
        message: String,
    },
    /// 不需要消费的事件。
    Ignored,
}

/// 解析一条服务端事件；非法 JSON 视为 `Ignored`，不让脏帧打断会话。
pub(super) fn parse_server_event(text: &str) -> ServerEvent {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return ServerEvent::Ignored;
    };
    let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) else {
        return ServerEvent::Ignored;
    };
    match kind {
        "session.created" => ServerEvent::SessionCreated,
        "session.updated" => ServerEvent::SessionUpdated,
        "input_audio_buffer.speech_started" => ServerEvent::SpeechStarted,
        "input_audio_buffer.speech_stopped" => ServerEvent::SpeechStopped,
        "conversation.item.input_audio_transcription.delta" => ServerEvent::InputTranscriptDelta {
            text: string_field(&value, "text"),
            stash: string_field(&value, "stash"),
        },
        "conversation.item.input_audio_transcription.completed" => {
            ServerEvent::InputTranscriptCompleted {
                transcript: string_field(&value, "transcript"),
            }
        }
        "response.created" => ServerEvent::ResponseCreated,
        "response.audio_transcript.delta" | "response.text.delta" => {
            ServerEvent::OutputTranscriptDelta {
                text: string_field(&value, "delta"),
            }
        }
        "response.audio_transcript.done" => ServerEvent::OutputTranscriptDone {
            transcript: string_field(&value, "transcript"),
        },
        "response.audio.delta" => {
            let encoded = string_field(&value, "delta");
            match base64::engine::general_purpose::STANDARD.decode(encoded) {
                Ok(pcm) => ServerEvent::AudioDelta { pcm },
                Err(_) => ServerEvent::Ignored,
            }
        }
        "response.done" => ServerEvent::ResponseDone,
        "error" => ServerEvent::Error {
            code: error_code(&value),
            message: error_message(&value),
        },
        _ => ServerEvent::Ignored,
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
