//! 火山方舟 Agent Plan 的流式识别：把二进制帧协议翻译成 [`HostTransport`]。
//!
//! 上层 `VoiceBridge` 只认 `Control` / `Frame`，本模块负责三件事：
//!
//! 1. **下行翻译**：`Warmup` 只做就绪确认、`SessionStart` 发首帧（含语言与音频
//!    参数）、`UtteranceEnd` 发末帧收尾、`Cancel` / `Close` 收线；
//! 2. **上行拆帧**：`send_bytes` 收到的是管道帧（`kind + len + PCM`），拆出裸
//!    PCM 后按火山的 gzip 二进制帧重新编码；
//! 3. **结果回译**：服务端的 JSON 结果翻回 `Partial` / `Final`，回合编排零改动。
//!
//! 传输是**同步**契约（worker 跑在 `std::thread` 上），而 `tokio-tungstenite`
//! 是异步的：这里持有一个单线程 runtime，每次 `block_on` 一小段，不引入第二个
//! 线程，也不改上层的同步循环。

use std::collections::VecDeque;
use std::io::Read as _;
use std::io::Write as _;
use std::time::{Duration, Instant};

use axon_voice_bridge::axon_voice_protocol::{
    Control, ErrorMsg, Final, Frame, Partial, Warm, decode_frame,
};
use axon_voice_bridge::{HostTransport, TransportError};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::http::header::HeaderName;
use tokio_tungstenite::tungstenite::protocol::Message;

/// 默认识别端点：Agent Plan 的流式识别通道。
pub(super) const DEFAULT_ASR_ENDPOINT: &str =
    "wss://openspeech.bytedance.com/api/v3/plan/sauc/bigmodel_nostream";
/// 默认资源 ID：流式识别按时长计费。
pub(super) const DEFAULT_ASR_RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
/// 默认模型：产品口径的标识，只用于上报与日志，协议里仍发 `bigmodel`。
pub(super) const DEFAULT_ASR_MODEL: &str = "doubao-seed-asr-2.0";
/// 默认语言：跟随配置，缺省中文。
pub(super) const DEFAULT_ASR_LANGUAGE: &str = "zh";

/// 建连超时：与预热合同值一致。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 首帧占用的序号；音频从它加一开始。
const FIRST_AUDIO_SEQ: i32 = 2;

/// 协议版本 1。
const PROTOCOL_VERSION: u8 = 0b0001;
/// 固定 4 字节头。
const HEADER_SIZE: u8 = 0b0001;
/// 完整客户端请求。
const MT_FULL_REQUEST: u8 = 0b0001;
/// 仅音频请求。
const MT_AUDIO_ONLY: u8 = 0b0010;
/// 服务端完整响应。
const MT_SERVER_RESPONSE: u8 = 0b1001;
/// 服务端确认。
const MT_SERVER_ACK: u8 = 0b1011;
/// 服务端错误。
const MT_SERVER_ERROR: u8 = 0b1111;
/// 带正序号。
const FLAG_POSITIVE_SEQ: u8 = 0b0001;
/// 末帧且不带序号。
const FLAG_LAST_NO_SEQ: u8 = 0b0010;
/// 末帧且带序号。
const FLAG_LAST_WITH_SEQ: u8 = 0b0011;
/// JSON 序列化。
const SERIALIZATION_JSON: u8 = 0b0001;
/// 不序列化（音频体）。
const SERIALIZATION_NONE: u8 = 0b0000;
/// gzip 压缩。
const COMPRESSION_GZIP: u8 = 0b0001;

/// 附加头：凭据。
const HEADER_API_KEY: HeaderName = HeaderName::from_static("x-api-key");
/// 附加头：资源 ID。
const HEADER_RESOURCE_ID: HeaderName = HeaderName::from_static("x-api-resource-id");
/// 附加头：连接标识。
const HEADER_CONNECT_ID: HeaderName = HeaderName::from_static("x-api-connect-id");

/// 一次识别所需的渠道参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolcAsrConfig {
    /// 完整 WS 地址。
    pub endpoint: String,
    /// `X-Api-Key` 凭据。
    pub api_key: String,
    /// `X-Api-Resource-Id` 资源标识。
    pub resource_id: String,
    /// 上报给运行态的模型标识。
    pub model: String,
    /// 转写语言，`auto` 表示由服务端判定。
    pub language: String,
}

impl VolcAsrConfig {
    /// 补齐空字段：语言缺省中文，模型缺省产品口径。
    #[must_use]
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        resource_id: impl Into<String>,
        model: impl Into<String>,
        language: impl Into<String>,
    ) -> Self {
        let model = model.into();
        let model = if model.trim().is_empty() {
            DEFAULT_ASR_MODEL.to_owned()
        } else {
            model
        };
        let language = language.into();
        let language = if language.trim().is_empty() {
            DEFAULT_ASR_LANGUAGE.to_owned()
        } else {
            language
        };
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            resource_id: resource_id.into(),
            model,
            language,
        }
    }
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// 火山识别通道：把二进制帧协议包装成同步传输。
///
/// `bigmodel_nostream` 是「一次连接一次请求」：首帧开始、末帧收尾、结果回完
/// 服务端即关连接。会话要连续听多句，所以每句收尾后由本模块退役旧连接，
/// 下一句开头按同一份配置重连，上层无感。
pub(super) struct VolcAsrTransport {
    /// 单线程 runtime：只用来驱动 WS 读写，不并发。
    runtime: tokio::runtime::Runtime,
    /// 建连参数：请求收尾后按同一份配置重连。
    config: VolcAsrConfig,
    /// 当前连接；两句之间为空，下一句开头再连。
    socket: Option<Socket>,
    /// 待交付给上层的本地帧（预热回执等）。
    pending: VecDeque<Frame>,
    /// 下一个音频帧序号。
    next_seq: i32,
    /// 当前句序号，`final` 回填用。
    utterance_seq: u32,
    /// 首帧要带的语言，来自 `SessionStart`。
    language: String,
    /// 当前是否有未收尾的请求（已发首帧、未发末帧）。
    request_open: bool,
    /// 是否已收线。
    closed: bool,
}

impl VolcAsrTransport {
    /// 建立连接；凭据错误会在握手期暴露，不拖到首帧。
    ///
    /// # Errors
    ///
    /// 端点非法、握手失败或超时。
    pub(super) fn open(config: &VolcAsrConfig) -> Result<Self, TransportError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| TransportError::Write(format!("创建识别运行时失败：{error}")))?;
        let mut transport = Self {
            runtime,
            config: config.clone(),
            socket: None,
            pending: VecDeque::new(),
            next_seq: FIRST_AUDIO_SEQ,
            utterance_seq: 0,
            language: config.language.clone(),
            request_open: false,
            closed: false,
        };
        transport.connect()?;
        Ok(transport)
    }

    /// 按配置建连；已有连接会被替换。
    fn connect(&mut self) -> Result<(), TransportError> {
        let mut request = self
            .config
            .endpoint
            .as_str()
            .into_client_request()
            .map_err(|error| TransportError::Write(format!("识别端点非法：{error}")))?;
        {
            let headers = request.headers_mut();
            let key = HeaderValue::from_str(&self.config.api_key)
                .map_err(|_| TransportError::Write("识别凭据含非法字符".to_owned()))?;
            headers.insert(HEADER_API_KEY, key);
            let resource = HeaderValue::from_str(&self.config.resource_id)
                .map_err(|_| TransportError::Write("识别资源标识含非法字符".to_owned()))?;
            headers.insert(HEADER_RESOURCE_ID, resource);
            let connect_id = HeaderValue::from_str(&uuid::Uuid::new_v4().to_string())
                .map_err(|_| TransportError::Write("连接标识生成失败".to_owned()))?;
            headers.insert(HEADER_CONNECT_ID, connect_id);
        }
        let connected = self
            .runtime
            .block_on(tokio::time::timeout(
                CONNECT_TIMEOUT,
                tokio_tungstenite::connect_async(request),
            ))
            .map_err(|_| TransportError::Timeout)?
            .map_err(|error| TransportError::Write(format!("识别握手失败：{error}")))?;
        self.socket = Some(connected.0);
        Ok(())
    }

    /// 开一次识别请求：无连接时先重连，再发首帧。
    fn begin_request(&mut self) -> Result<(), TransportError> {
        if self.socket.is_none() {
            self.connect()?;
        }
        self.next_seq = FIRST_AUDIO_SEQ;
        let language = self.language.clone();
        self.send_full_request(&language)?;
        self.request_open = true;
        Ok(())
    }

    /// 退役当前连接：一次 `nostream` 请求到此为止，下一句重新建连。
    fn retire_socket(&mut self) {
        self.socket = None;
        self.request_open = false;
    }

    /// 读一帧服务端消息；`Ok(None)` 表示对端正常关流。
    fn next_message(&mut self, timeout: Duration) -> Result<Option<Message>, TransportError> {
        let Some(socket) = self.socket.as_mut() else {
            return Err(TransportError::Timeout);
        };
        let runtime = &self.runtime;
        match runtime.block_on(tokio::time::timeout(timeout, socket.next())) {
            Ok(Some(Ok(message))) => Ok(Some(message)),
            Ok(Some(Err(error))) => Err(TransportError::Read(format!("识别下行失败：{error}"))),
            Ok(None) => Ok(None),
            Err(_) => Err(TransportError::Timeout),
        }
    }

    /// 发关闭帧并丢弃连接；连接已为空时什么都不做。
    fn close_socket(&mut self) {
        if let Some(mut socket) = self.socket.take() {
            let runtime = &self.runtime;
            let _ = runtime.block_on(socket.send(Message::Close(None)));
        }
    }

    /// 发送首帧：语言、音频参数与结果形态都在这里定死。
    fn send_full_request(&mut self, language: &str) -> Result<(), TransportError> {
        let mut request = serde_json::json!({
            "model_name": "bigmodel",
            "enable_itn": true,
            "enable_punc": true,
            "enable_ddc": true,
            "show_utterances": true,
            "result_type": "full",
        });
        if !language.eq_ignore_ascii_case("auto") && !language.trim().is_empty() {
            request["language"] = serde_json::json!(language);
        }
        let payload = serde_json::json!({
            "user": { "uid": "axon" },
            "audio": { "format": "pcm", "codec": "raw", "rate": 16_000, "bits": 16, "channel": 1 },
            "request": request,
        });
        let encoded = serde_json::to_vec(&payload)
            .map_err(|error| TransportError::Write(format!("识别首帧序列化失败：{error}")))?;
        let body = gzip(&encoded)?;
        let mut frame = header(
            MT_FULL_REQUEST,
            FLAG_POSITIVE_SEQ,
            SERIALIZATION_JSON,
            COMPRESSION_GZIP,
        );
        frame.extend_from_slice(&1i32.to_be_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        self.send_binary(frame)
    }

    /// 发送一片上行音频；`last` 为末帧标记。
    fn send_audio(&mut self, pcm: &[u8], last: bool) -> Result<(), TransportError> {
        // 末帧用负序号标记，与服务端约定一致。
        let seq = if last {
            -self.next_seq
        } else {
            self.next_seq
        };
        self.next_seq = self.next_seq.saturating_add(1);
        let body = if pcm.is_empty() {
            Vec::new()
        } else {
            gzip(pcm)?
        };
        let flags = if last {
            FLAG_LAST_WITH_SEQ
        } else {
            FLAG_POSITIVE_SEQ
        };
        let mut frame = header(MT_AUDIO_ONLY, flags, SERIALIZATION_NONE, COMPRESSION_GZIP);
        frame.extend_from_slice(&seq.to_be_bytes());
        frame.extend_from_slice(&(body.len() as u32).to_be_bytes());
        frame.extend_from_slice(&body);
        self.send_binary(frame)
    }

    fn send_binary(&mut self, bytes: Vec<u8>) -> Result<(), TransportError> {
        let Some(socket) = self.socket.as_mut() else {
            return Err(TransportError::Closed);
        };
        let runtime = &self.runtime;
        runtime
            .block_on(socket.send(Message::Binary(bytes.into())))
            .map_err(|error| TransportError::Write(format!("识别上行失败：{error}")))
    }

    /// 解析一帧服务端消息；无事件可交付时返回 `None`。
    fn parse_server(&mut self, bytes: &[u8]) -> Result<Option<Frame>, TransportError> {
        if bytes.len() < 4 {
            return Err(TransportError::Read("识别帧过短".to_owned()));
        }
        let header_size = usize::from(bytes[0] & 0x0F) * 4;
        let message_type = bytes[1] >> 4;
        let flags = bytes[1] & 0x0F;
        let compression = bytes[2] & 0x0F;
        if header_size > bytes.len() {
            return Err(TransportError::Read("识别帧头越界".to_owned()));
        }
        let mut offset = header_size;
        if matches!(flags, FLAG_POSITIVE_SEQ | FLAG_LAST_WITH_SEQ) {
            offset += 4;
        }
        if message_type == MT_SERVER_ACK {
            return Ok(None);
        }
        if message_type == MT_SERVER_ERROR {
            let (code, _message) = read_error(bytes, offset, compression);
            return Ok(Some(Frame::Control(Control::Error(ErrorMsg {
                code: format!("asr_{code}"),
                stage: "listening".to_owned(),
                retryable: false,
                detail: None,
            }))));
        }
        if message_type != MT_SERVER_RESPONSE {
            return Ok(None);
        }
        let Some(size) = read_u32(bytes, offset) else {
            return Ok(None);
        };
        offset += 4;
        let end = offset.saturating_add(size as usize);
        let Some(body) = bytes.get(offset..end) else {
            return Ok(None);
        };
        let raw = if compression == COMPRESSION_GZIP && !body.is_empty() {
            let mut decoder = GzDecoder::new(body);
            let mut out = Vec::new();
            decoder
                .read_to_end(&mut out)
                .map_err(|error| TransportError::Read(format!("识别结果解压失败：{error}")))?;
            out
        } else {
            body.to_vec()
        };
        let value: serde_json::Value = match serde_json::from_slice(&raw) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let is_last = matches!(flags, FLAG_LAST_NO_SEQ | FLAG_LAST_WITH_SEQ);
        let text = value
            .get("result")
            .and_then(|result| result.get("text"))
            .and_then(|text| text.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .to_owned();
        if text.is_empty() && !is_last {
            return Ok(None);
        }
        if is_last {
            let (start_ms, end_ms) = utterance_span(&value);
            // 句序号沿用上层在 `UtteranceEnd` 里给的值：一请求一句，不自行递增。
            let utterance_seq = self.utterance_seq;
            // 结果已收完，服务端会关连接；主动退役，下一句重新建连。
            self.retire_socket();
            return Ok(Some(Frame::Control(Control::Final(Final {
                utterance_seq,
                text,
                start_ms,
                end_ms,
                finalize_ms: end_ms,
            }))));
        }
        Ok(Some(Frame::Control(Control::Partial(Partial {
            utterance_seq: self.utterance_seq,
            text,
        }))))
    }
}

impl HostTransport for VolcAsrTransport {
    fn send_control(&mut self, control: &Control) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        match control {
            // 建连在 `open` 完成，预热只需回执；回执由 `recv` 交付。
            Control::Warmup(warmup) => {
                self.pending.push_back(Frame::Control(Control::Warm(Warm {
                    family: warmup.family.clone(),
                    model: warmup.model.clone(),
                    rtf_p50: 0.0,
                    rss_kb: 0,
                })));
                Ok(())
            }
            Control::SessionStart(start) => {
                if !start.lang.trim().is_empty() {
                    self.language = start.lang.clone();
                }
                self.utterance_seq = 0;
                // 会话一开始就发首帧：连接是 `open` 时建好的，中间不留空闲窗口。
                self.begin_request()
            }
            Control::UtteranceEnd(end) => {
                self.utterance_seq = end.utterance_seq;
                if !self.request_open {
                    return Ok(());
                }
                self.send_audio(&[], true)?;
                // 请求已收尾，连接留给 `recv` 收结果，收完再退役。
                self.request_open = false;
                Ok(())
            }
            Control::Cancel(_) => {
                // 取消当前轮：能收尾就收尾，随后退役连接，等下一句重连。
                if self.request_open {
                    self.send_audio(&[], true).ok();
                }
                self.retire_socket();
                Ok(())
            }
            Control::Close => {
                if self.request_open {
                    self.send_audio(&[], true).ok();
                }
                self.close_socket();
                self.closed = true;
                Ok(())
            }
            // 其余控制（问候、心跳、朗读稿、会话结束）与识别服务无关。
            _ => Ok(()),
        }
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        if self.closed {
            return Err(TransportError::Closed);
        }
        let frame = decode_frame(bytes)
            .map_err(|error| TransportError::Write(format!("管道帧非法：{error}")))?;
        let Frame::AudioIn(pcm) = frame else {
            return Err(TransportError::Write("识别只接受上行音频帧".to_owned()));
        };
        // 上一句已收尾时先开新请求：一次连接只服务一个请求。
        if !self.request_open {
            self.begin_request()?;
        }
        self.send_audio(&pcm, false)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame, TransportError> {
        if let Some(frame) = self.pending.pop_front() {
            return Ok(frame);
        }
        if self.closed {
            return Err(TransportError::Closed);
        }
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(TransportError::Timeout);
            }
            let message = match self.next_message(remaining) {
                Ok(Some(message)) => message,
                Ok(None) => {
                    // 服务端关连接：请求进行中属异常，两句之间属正常收尾。
                    if self.request_open {
                        self.closed = true;
                        return Ok(Frame::Control(Control::Closed));
                    }
                    self.retire_socket();
                    return Err(TransportError::Timeout);
                }
                Err(TransportError::Timeout) => return Err(TransportError::Timeout),
                Err(error) => {
                    if !self.request_open {
                        self.retire_socket();
                        return Err(TransportError::Timeout);
                    }
                    self.closed = true;
                    return Err(error);
                }
            };
            match message {
                Message::Binary(bytes) => {
                    if let Some(frame) = self.parse_server(&bytes)? {
                        return Ok(frame);
                    }
                }
                Message::Close(_) => {
                    if self.request_open {
                        self.closed = true;
                        return Ok(Frame::Control(Control::Closed));
                    }
                    self.retire_socket();
                    return Err(TransportError::Timeout);
                }
                _ => {}
            }
        }
    }

    fn heartbeat_ok(&self, _now: Instant) -> bool {
        !self.closed
    }

    fn alive(&mut self) -> bool {
        !self.closed
    }

    fn terminate(&mut self) -> Option<i32> {
        None
    }

    fn shutdown(&mut self) {
        if self.closed {
            return;
        }
        if self.request_open {
            self.send_audio(&[], true).ok();
        }
        self.close_socket();
        self.closed = true;
    }
}

/// 构造 4 字节协议头。
fn header(message_type: u8, flags: u8, serialization: u8, compression: u8) -> Vec<u8> {
    vec![
        (PROTOCOL_VERSION << 4) | HEADER_SIZE,
        (message_type << 4) | flags,
        (serialization << 4) | compression,
        0x00,
    ]
}

/// gzip 压缩。
fn gzip(bytes: &[u8]) -> Result<Vec<u8>, TransportError> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|error| TransportError::Write(format!("识别压缩失败：{error}")))?;
    encoder
        .finish()
        .map_err(|error| TransportError::Write(format!("识别压缩收尾失败：{error}")))
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// 读错误码与说明，尽力而为：解压失败时退回原始字节。
fn read_error(bytes: &[u8], offset: usize, compression: u8) -> (u32, String) {
    let Some(code) = read_u32(bytes, offset) else {
        return (0, String::new());
    };
    let Some(size) = read_u32(bytes, offset + 4) else {
        return (code, String::new());
    };
    let start = offset + 8;
    let Some(body) = bytes.get(start..start.saturating_add(size as usize)) else {
        return (code, String::new());
    };
    let raw = if compression == COMPRESSION_GZIP && !body.is_empty() {
        let mut decoder = GzDecoder::new(body);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_or_else(|_| body.to_vec(), |_| out)
    } else {
        body.to_vec()
    };
    (code, String::from_utf8_lossy(&raw).into_owned())
}

/// 取首句起点与末句终点；缺失时为 0。
fn utterance_span(value: &serde_json::Value) -> (u64, u64) {
    let Some(items) = value
        .get("result")
        .and_then(|result| result.get("utterances"))
        .and_then(|items| items.as_array())
    else {
        return (0, 0);
    };
    let start = items
        .first()
        .and_then(|item| item.get("start_time"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let end = items
        .iter()
        .rev()
        .find_map(|item| item.get("end_time").and_then(serde_json::Value::as_u64))
        .unwrap_or(start);
    (start, end)
}

#[cfg(test)]
#[path = "voice_asr_volc_tests.rs"]
mod tests;
