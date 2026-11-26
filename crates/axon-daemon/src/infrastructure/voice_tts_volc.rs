//! 火山方舟 Agent Plan 的流式合成：一条 WebSocket，文本进、WAV 出。
//!
//! 协议是二进制帧而不是 JSON 文本帧：4 字节头 + 可选序列号 + 可选事件号 +
//! 长度前缀的载荷。请求、音频分片与错误各占一种 message type，解析时按头部
//! 字段分流。这里只做「一次请求一次收线」：合成整段音频再交给上层，逐段
//! 预取与播放排程仍由既有 segments / streamPlayer 负责，不动播放模型。

use axon_rustls_provider::ensure_rustls_crypto_provider;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::HeaderName;
use tokio_tungstenite::tungstenite::protocol::Message;

/// 默认合成端点：Agent Plan 的单向流式通道。
pub(super) const DEFAULT_TTS_ENDPOINT: &str =
    "wss://openspeech.bytedance.com/api/v3/plan/tts/unidirectional/stream";
/// 默认资源 ID：seed-tts 2.0。
pub(super) const DEFAULT_TTS_RESOURCE_ID: &str = "seed-tts-2.0";
/// 默认音色：爽快思思，与「干练利落」的人格基线一致。
pub(super) const DEFAULT_TTS_SPEAKER: &str = "zh_female_shuangkuaisisi_uranus_bigtts";

/// 合成采样率：24 kHz，播放端自行重采样到 16 kHz。
const SAMPLE_RATE: u32 = 24_000;
/// 一次性合成的整体超时。
const SYNTH_TIMEOUT: Duration = Duration::from_secs(30);
/// 请求帧的固定头：version 1、header size 4、full client request、JSON + 无压缩。
const REQUEST_HEADER: [u8; 4] = [0x11, 0x10, 0x10, 0x00];
/// 结束事件：收到即收线。
const EVENT_FINISHED: i32 = 152;
/// 消息类型：音频分片。
const MT_AUDIO_ONLY: u8 = 0x0B;
/// 消息类型：JSON 载荷。
const MT_FULL_RESPONSE: u8 = 0x09;
/// 消息类型：错误。
const MT_ERROR: u8 = 0x0F;
/// 标志位：带序列号。
const FLAG_SEQUENCE: u8 = 0x01;
/// 标志位：带事件号。
const FLAG_EVENT: u8 = 0x04;
/// 附加头：资源 ID。
const HEADER_RESOURCE_ID: HeaderName = HeaderName::from_static("x-api-resource-id");
/// 附加头：凭据。
const HEADER_API_KEY: HeaderName = HeaderName::from_static("x-api-key");
/// 附加头：连接标识。
const HEADER_CONNECT_ID: HeaderName = HeaderName::from_static("x-api-connect-id");

/// 一次合成所需的渠道参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolcTtsConfig {
    /// 完整 WS 地址。
    pub endpoint: String,
    /// `X-Api-Key` 凭据。
    pub api_key: String,
    /// `X-Api-Resource-Id` 资源标识。
    pub resource_id: String,
    /// 音色标识。
    pub speaker: String,
    /// 语速，`-50..=100`，0 表示服务端默认。
    pub speech_rate: i32,
}

/// 合成失败的原因；上层按码映射成对外错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TtsError {
    /// 连不上或中途断开。
    Transport,
    /// 服务端明确报错。
    Provider,
    /// 超时。
    Timeout,
    /// 一帧音频都没收到。
    Empty,
}

impl TtsError {
    /// 对外错误码。
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::Transport | Self::Timeout => "worker_unavailable",
            Self::Provider | Self::Empty => "tts_failed",
        }
    }
}

/// 合成一段文本，返回可直接下发的 WAV 字节。
///
/// # Errors
///
/// 连接失败、服务端报错、超时或未产出音频时返回对应原因。
pub(super) async fn synth_wav(cfg: &VolcTtsConfig, text: &str) -> Result<Vec<u8>, TtsError> {
    let request = build_request(cfg)?;
    ensure_rustls_crypto_provider();
    let connected = tokio::time::timeout(SYNTH_TIMEOUT, tokio_tungstenite::connect_async(request))
        .await
        .map_err(|_| TtsError::Timeout)?;
    let (socket, _) = connected.map_err(|_| TtsError::Transport)?;
    let (mut sink, mut stream) = socket.split();
    if sink
        .send(Message::Binary(
            request_frame(text, &cfg.speaker, cfg.speech_rate).into(),
        ))
        .await
        .is_err()
    {
        return Err(TtsError::Transport);
    }

    let mut audio: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + SYNTH_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(TtsError::Timeout);
        }
        let next = tokio::time::timeout(remaining, stream.next())
            .await
            .map_err(|_| TtsError::Timeout)?;
        let frame = match next {
            Some(Ok(Message::Binary(bytes))) => bytes,
            Some(Ok(Message::Close(_))) | None => break,
            // 文本帧与 ping/pong 不是协议载荷，忽略。
            Some(Ok(_)) => continue,
            Some(Err(_)) => return Err(TtsError::Transport),
        };
        match parse_frame(&frame) {
            ServerFrame::Audio(chunk) => audio.extend_from_slice(&chunk),
            ServerFrame::Error(_) => return Err(TtsError::Provider),
            ServerFrame::Event(EVENT_FINISHED) => break,
            ServerFrame::Event(_) | ServerFrame::Other => {}
        }
    }
    let _ = sink.close().await;
    if audio.is_empty() {
        return Err(TtsError::Empty);
    }
    Ok(repair_wav(audio))
}

/// 一帧服务端消息的归类。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerFrame {
    /// 音频分片。
    Audio(Vec<u8>),
    /// 带事件号的 JSON 帧。
    Event(i32),
    /// 错误帧。
    Error(i32),
    /// 其它帧（握手、文本帧等）。
    Other,
}

/// 构造合成请求帧：固定头 + 载荷长度 + JSON 载荷。
fn request_frame(text: &str, speaker: &str, speech_rate: i32) -> Vec<u8> {
    let mut audio_params = serde_json::json!({
        "format": "wav",
        "sample_rate": SAMPLE_RATE,
    });
    if speech_rate != 0 {
        audio_params["speech_rate"] = serde_json::json!(speech_rate);
    }
    let payload = serde_json::json!({
        "req_params": {
            "text": text,
            "speaker": speaker,
            "audio_params": audio_params,
        }
    })
    .to_string();
    let body = payload.as_bytes();
    let mut frame = Vec::with_capacity(REQUEST_HEADER.len() + 4 + body.len());
    frame.extend_from_slice(&REQUEST_HEADER);
    frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
    frame.extend_from_slice(body);
    frame
}

/// 解析一帧服务端消息；字段不足时按「其它」处理，不 panic。
fn parse_frame(frame: &[u8]) -> ServerFrame {
    if frame.len() < 4 {
        return ServerFrame::Other;
    }
    let header_size = usize::from(frame[0] & 0x0F) * 4;
    let message_type = frame[1] >> 4;
    let flags = frame[1] & 0x0F;
    if header_size > frame.len() {
        return ServerFrame::Other;
    }
    let mut offset = header_size;
    if flags & FLAG_SEQUENCE != 0 {
        offset += 4;
    }
    let event = if flags & FLAG_EVENT != 0 {
        let Some(value) = read_i32(frame, offset) else {
            return ServerFrame::Other;
        };
        offset += 4;
        Some(value)
    } else {
        None
    };
    match message_type {
        MT_AUDIO_ONLY => {
            // 音频帧先有一个 session 串，再是音频长度。
            let Some(session_len) = read_u32(frame, offset) else {
                return ServerFrame::Other;
            };
            offset += 4 + session_len as usize;
            let Some(audio_len) = read_u32(frame, offset) else {
                return ServerFrame::Other;
            };
            offset += 4;
            let end = offset.saturating_add(audio_len as usize);
            if end > frame.len() {
                return ServerFrame::Other;
            }
            ServerFrame::Audio(frame[offset..end].to_vec())
        }
        MT_ERROR => read_i32(frame, offset).map_or(ServerFrame::Other, ServerFrame::Error),
        MT_FULL_RESPONSE => event.map_or(ServerFrame::Other, ServerFrame::Event),
        _ => event.map_or(ServerFrame::Other, ServerFrame::Event),
    }
}

fn read_u32(frame: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let slice = frame.get(offset..end)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_i32(frame: &[u8], offset: usize) -> Option<i32> {
    let end = offset.checked_add(4)?;
    let slice = frame.get(offset..end)?;
    Some(i32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// 回填流式 WAV 的长度字段。
///
/// 服务端按流式产出，RIFF 与 `data` 的长度先写占位 `0xFFFFFFFF`；客户端收齐后
/// 必须按实际字节数改写，否则桌面播放器按长度预分配会读出噪音。
fn repair_wav(mut wav: Vec<u8>) -> Vec<u8> {
    if wav.len() < 12 || &wav[0..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return wav;
    }
    let total = wav.len() as u32;
    wav[4..8].copy_from_slice(&(total.saturating_sub(8)).to_le_bytes());
    // 顺序扫 chunk，找到 data 并把长度改成剩余字节数。
    let mut offset = 12usize;
    while offset + 8 <= wav.len() {
        let id = [
            wav[offset],
            wav[offset + 1],
            wav[offset + 2],
            wav[offset + 3],
        ];
        let size = u32::from_le_bytes([
            wav[offset + 4],
            wav[offset + 5],
            wav[offset + 6],
            wav[offset + 7],
        ]);
        if &id == b"data" {
            let real = (wav.len() - offset - 8) as u32;
            wav[offset + 4..offset + 8].copy_from_slice(&real.to_le_bytes());
            return wav;
        }
        offset = offset.saturating_add(8).saturating_add(size as usize);
    }
    wav
}

/// 构造握手请求。
fn build_request(
    cfg: &VolcTtsConfig,
) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, TtsError> {
    let mut request = cfg
        .endpoint
        .as_str()
        .into_client_request()
        .map_err(|_| TtsError::Transport)?;
    let headers = request.headers_mut();
    let key = HeaderValue::from_str(&cfg.api_key).map_err(|_| TtsError::Transport)?;
    headers.insert(HEADER_API_KEY, key);
    let resource = HeaderValue::from_str(&cfg.resource_id).map_err(|_| TtsError::Transport)?;
    headers.insert(HEADER_RESOURCE_ID, resource);
    let connect_id = HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
        .map_err(|_| TtsError::Transport)?;
    headers.insert(HEADER_CONNECT_ID, connect_id);
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::{REQUEST_HEADER, ServerFrame, parse_frame, repair_wav, request_frame};

    fn text_frame(event: i32) -> Vec<u8> {
        let mut frame = vec![0x11, 0x94, 0x10, 0x00];
        frame.extend_from_slice(&event.to_be_bytes());
        frame.extend_from_slice(&4u32.to_be_bytes());
        frame.extend_from_slice(&0u32.to_be_bytes());
        frame
    }

    #[test]
    fn request_frame_carries_text_speaker_and_length() {
        let frame = request_frame("你好", "speaker", 0);
        assert_eq!(&frame[..4], &REQUEST_HEADER);
        let len = u32::from_be_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
        assert_eq!(len, frame.len() - 8, "长度前缀必须等于载荷长度");
        let body = String::from_utf8(frame[8..].to_vec()).expect("载荷是 UTF-8 JSON");
        assert!(body.contains("\"你好\""));
        assert!(body.contains("\"speaker\""));
        assert!(body.contains("\"wav\""));
        assert!(!body.contains("speech_rate"), "0 语速不写进请求");
    }

    #[test]
    fn request_frame_writes_non_zero_speech_rate() {
        let frame = request_frame("你好", "speaker", 20);
        let body = String::from_utf8(frame[8..].to_vec()).expect("载荷是 UTF-8 JSON");
        assert!(body.contains("\"speech_rate\":20"));
    }

    #[test]
    fn parse_frame_reads_audio_chunk() {
        let mut frame = vec![0x11, 0xB4, 0x10, 0x00];
        frame.extend_from_slice(&352i32.to_be_bytes());
        frame.extend_from_slice(&0u32.to_be_bytes()); // 空 session 串
        frame.extend_from_slice(&3u32.to_be_bytes());
        frame.extend_from_slice(&[1, 2, 3]);
        assert_eq!(parse_frame(&frame), ServerFrame::Audio(vec![1, 2, 3]));
    }

    #[test]
    fn parse_frame_reads_finished_event() {
        assert_eq!(parse_frame(&text_frame(152)), ServerFrame::Event(152));
    }

    #[test]
    fn parse_frame_reads_error_code() {
        // 错误帧不带事件号：头部之后直接是错误码。
        let mut frame = vec![0x11, 0xF0, 0x10, 0x00];
        frame.extend_from_slice(&ERROR_CODE.to_be_bytes());
        assert_eq!(parse_frame(&frame), ServerFrame::Error(ERROR_CODE));
    }

    #[test]
    fn parse_frame_ignores_truncated_input() {
        assert_eq!(parse_frame(&[]), ServerFrame::Other);
        assert_eq!(parse_frame(&[0x11, 0xB4]), ServerFrame::Other);
        assert_eq!(parse_frame(&[0x11, 0xB4, 0x10, 0x00]), ServerFrame::Other);
    }

    #[test]
    fn repair_wav_fills_riff_and_data_lengths() {
        let mut wav = b"RIFF\xff\xff\xff\xffWAVEfmt ".to_vec();
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt 长度
        wav.extend_from_slice(&[0u8; 16]); // fmt 内容
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        wav.extend_from_slice(&[7u8; 10]);
        let fixed = repair_wav(wav);
        let riff = u32::from_le_bytes([fixed[4], fixed[5], fixed[6], fixed[7]]);
        assert_eq!(
            riff as usize,
            fixed.len() - 8,
            "RIFF 长度必须是文件长度减 8"
        );
        let data_start = 12 + 8 + 16;
        let data_len = u32::from_le_bytes([
            fixed[data_start + 4],
            fixed[data_start + 5],
            fixed[data_start + 6],
            fixed[data_start + 7],
        ]);
        assert_eq!(data_len, 10, "data 长度必须是实际音频字节数");
    }

    #[test]
    fn repair_wav_leaves_non_riff_bytes_untouched() {
        let raw = vec![0u8; 32];
        assert_eq!(repair_wav(raw.clone()), raw);
    }

    /// 错误码样例：与协议里的服务端错误码同量级。
    const ERROR_CODE: i32 = 45_000_000;

    /// 真实合成冒烟：需要本机 `ARK_KEY` 与出网，默认跳过。
    ///
    /// 手动执行：`cargo test -p axon-daemon --lib live_synthesis -- --ignored`
    #[tokio::test]
    #[ignore = "需要 ARK_KEY 与网络，按需手动执行"]
    async fn live_synthesis_returns_playable_wav() {
        let Ok(api_key) = std::env::var("ARK_KEY") else {
            return;
        };
        let config = super::VolcTtsConfig {
            endpoint: super::DEFAULT_TTS_ENDPOINT.to_owned(),
            api_key,
            resource_id: super::DEFAULT_TTS_RESOURCE_ID.to_owned(),
            speaker: super::DEFAULT_TTS_SPEAKER.to_owned(),
            speech_rate: 0,
        };
        let wav = super::synth_wav(&config, "你好，我是知微。")
            .await
            .expect("合成应当成功");
        assert_eq!(&wav[0..4], b"RIFF", "必须是 RIFF 容器");
        assert_eq!(&wav[8..12], b"WAVE", "必须是 WAVE 格式");
        let riff = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]) as usize;
        assert_eq!(riff, wav.len() - 8, "RIFF 长度必须是真实文件长度");
        assert!(wav.len() > 10_000, "音频太短，可能没真正合成");
    }
}
