//! 最小 WebSocket 客户端（RFC 6455 子集）。
//!
//! 只为「宿主连本机 sherpa-onnx 流式识别服务」这一条链路服务：明文 TCP、无扩展、
//! 无压缩、不需要子协议协商；留在这里而不是引入新依赖，是为了让宿主 crate 的依赖面
//! 保持为零（合同 §4.4 只允许原生推理绑定）。
//!
//! 刻意不做两件事，并在代码里说明原因：
//! - 不校验 `Sec-WebSocket-Accept` 摘要：那需要再实现一份 SHA-1，而本链路对端是
//!   本进程监听的 127.0.0.1，握手只用于确认「对端确实按 WS 协议应答」。
//! - 不支持分片出站：我们只发整帧；入站分片按 RFC 拼接。

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// 续帧。
const OPCODE_CONTINUATION: u8 = 0x0;
/// 文本帧。
const OPCODE_TEXT: u8 = 0x1;
/// 二进制帧。
const OPCODE_BINARY: u8 = 0x2;
/// 关闭帧。
const OPCODE_CLOSE: u8 = 0x8;
/// Ping。
const OPCODE_PING: u8 = 0x9;
/// Pong。
const OPCODE_PONG: u8 = 0xA;
/// 单帧载荷上限：识别服务的 JSON 不会超过这个量级。
const MAX_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024;
/// 握手响应头上限。
const MAX_HANDSHAKE_BYTES: usize = 8 * 1024;

/// WS 客户端错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsError {
    /// 连接或读写失败。
    Io(String),
    /// 握手被拒或响应不合法。
    Handshake(String),
    /// 帧格式不合法。
    Protocol(String),
    /// 对端关闭。
    Closed,
}

impl std::fmt::Display for WsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(reason) => write!(formatter, "WebSocket 读写失败：{reason}"),
            Self::Handshake(reason) => write!(formatter, "WebSocket 握手失败：{reason}"),
            Self::Protocol(reason) => write!(formatter, "WebSocket 帧不合法：{reason}"),
            Self::Closed => formatter.write_str("WebSocket 已被对端关闭"),
        }
    }
}

/// 一条入站消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsMessage {
    /// 文本。
    Text(String),
    /// 二进制。
    Binary(Vec<u8>),
    /// 对端关闭。
    Close,
}

/// 最小 WS 客户端。
pub struct WsClient {
    stream: TcpStream,
    peer: SocketAddr,
    mask_state: u64,
}

impl WsClient {
    /// 建立连接并完成握手。
    ///
    /// # Errors
    ///
    /// 连接失败、握手被拒或超时。
    pub fn connect(
        address: SocketAddr,
        host_header: &str,
        path: &str,
        timeout: Duration,
    ) -> Result<Self, WsError> {
        let stream = TcpStream::connect_timeout(&address, timeout)
            .map_err(|error| WsError::Io(error.to_string()))?;
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|()| stream.set_write_timeout(Some(timeout)))
            .and_then(|()| stream.set_nodelay(true))
            .map_err(|error| WsError::Io(error.to_string()))?;
        let mut client = Self {
            stream,
            peer: address,
            mask_state: seed(),
        };
        client.handshake(host_header, path)?;
        Ok(client)
    }

    /// 对端地址（端口即识别服务端口）。
    #[must_use]
    pub fn peer_port(&self) -> Option<u16> {
        Some(self.peer.port())
    }

    /// 发送文本帧。
    ///
    /// # Errors
    ///
    /// 写失败或连接已关闭。
    pub fn send_text(&mut self, text: &str) -> Result<(), WsError> {
        self.send_frame(OPCODE_TEXT, text.as_bytes())
    }

    /// 发送二进制帧。
    ///
    /// # Errors
    ///
    /// 写失败或连接已关闭。
    pub fn send_binary(&mut self, payload: &[u8]) -> Result<(), WsError> {
        self.send_frame(OPCODE_BINARY, payload)
    }

    /// 在预算内等待一条消息；预算内没有完整消息时返回 `Ok(None)`。
    ///
    /// # Errors
    ///
    /// 帧格式错误、读失败或连接关闭。
    pub fn read_message(&mut self, budget: Duration) -> Result<Option<WsMessage>, WsError> {
        if !self.wait_readable(budget)? {
            return Ok(None);
        }
        // 已经有数据可读，按帧读完整；单帧读取给足余量，避免再被预算切半。
        self.stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|error| WsError::Io(error.to_string()))?;
        let mut accumulated: Vec<u8> = Vec::new();
        let mut accumulated_kind: Option<u8> = None;
        loop {
            let (fin, opcode, payload) = self.read_frame()?;
            match opcode {
                OPCODE_PING => {
                    self.send_frame(OPCODE_PONG, &payload)?;
                }
                OPCODE_PONG => {}
                OPCODE_CLOSE => return Ok(Some(WsMessage::Close)),
                OPCODE_TEXT | OPCODE_BINARY | OPCODE_CONTINUATION => {
                    if opcode != OPCODE_CONTINUATION {
                        accumulated_kind = Some(opcode);
                        accumulated.clear();
                    }
                    accumulated.extend_from_slice(&payload);
                    if fin {
                        return Ok(Some(match accumulated_kind {
                            Some(OPCODE_BINARY) => WsMessage::Binary(accumulated),
                            _ => {
                                WsMessage::Text(String::from_utf8_lossy(&accumulated).into_owned())
                            }
                        }));
                    }
                }
                other => return Err(WsError::Protocol(format!("未知 opcode {other}"))),
            }
        }
    }

    /// 关闭连接（尽力而为，错误忽略）。
    pub fn close(&mut self) {
        let _ = self.send_frame(OPCODE_CLOSE, &[]);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    fn handshake(&mut self, host_header: &str, path: &str) -> Result<(), WsError> {
        let key = base64(&mask_bytes(seed()));
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_header}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        self.stream
            .write_all(request.as_bytes())
            .map_err(|error| WsError::Io(error.to_string()))?;
        let mut buffer = Vec::with_capacity(512);
        let mut byte = [0u8; 1];
        while !buffer.ends_with(b"\r\n\r\n") {
            if buffer.len() > MAX_HANDSHAKE_BYTES {
                return Err(WsError::Handshake("响应头超长".to_owned()));
            }
            match self.stream.read(&mut byte) {
                Ok(0) => return Err(WsError::Handshake("连接在握手阶段关闭".to_owned())),
                Ok(_) => buffer.push(byte[0]),
                Err(error) => return Err(WsError::Io(error.to_string())),
            }
        }
        let head = String::from_utf8_lossy(&buffer).to_ascii_lowercase();
        if !head.starts_with("http/1.1 101") {
            let first = head.lines().next().unwrap_or_default().to_owned();
            return Err(WsError::Handshake(first));
        }
        if !head.contains("sec-websocket-accept") {
            return Err(WsError::Handshake("缺少 Sec-WebSocket-Accept".to_owned()));
        }
        Ok(())
    }

    fn send_frame(&mut self, opcode: u8, payload: &[u8]) -> Result<(), WsError> {
        let mut frame = Vec::with_capacity(payload.len() + 14);
        frame.push(0x80 | opcode);
        let length = payload.len();
        if length < 126 {
            frame.push(0x80 | length as u8);
        } else if length <= usize::from(u16::MAX) {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        } else {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
        let mask = self.next_mask();
        frame.extend_from_slice(&mask);
        let key = mask;
        frame.extend(
            payload
                .iter()
                .enumerate()
                .map(|(index, byte)| byte ^ key[index % key.len()]),
        );
        self.stream
            .write_all(&frame)
            .map_err(|error| WsError::Io(error.to_string()))
    }

    fn read_frame(&mut self) -> Result<(bool, u8, Vec<u8>), WsError> {
        let mut head = [0u8; 2];
        self.read_exact(&mut head)?;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0F;
        let masked = head[1] & 0x80 != 0;
        let short_length = u64::from(head[1] & 0x7F);
        let length = match short_length {
            126 => {
                let mut extended = [0u8; 2];
                self.read_exact(&mut extended)?;
                u64::from(u16::from_be_bytes(extended))
            }
            127 => {
                let mut extended = [0u8; 8];
                self.read_exact(&mut extended)?;
                u64::from_be_bytes(extended)
            }
            other => other,
        };
        if length > MAX_PAYLOAD_BYTES {
            return Err(WsError::Protocol(format!("载荷过大：{length} 字节")));
        }
        let mask = if masked {
            let mut key = [0u8; 4];
            self.read_exact(&mut key)?;
            Some(key)
        } else {
            None
        };
        let mut payload = vec![0u8; length as usize];
        self.read_exact(&mut payload)?;
        if let Some(key) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= key[index % key.len()];
            }
        }
        Ok((fin, opcode, payload))
    }

    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), WsError> {
        match self.stream.read_exact(buffer) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => Err(WsError::Closed),
            Err(error) => Err(WsError::Io(error.to_string())),
        }
    }

    /// 轮询「现在有没有数据可读」：`peek` 不消费字节，因此不会把帧读半截。
    fn wait_readable(&mut self, budget: Duration) -> Result<bool, WsError> {
        self.stream
            .set_read_timeout(Some(budget.max(Duration::from_millis(1))))
            .map_err(|error| WsError::Io(error.to_string()))?;
        let mut probe = [0u8; 1];
        match self.stream.peek(&mut probe) {
            Ok(0) => Err(WsError::Closed),
            Ok(_) => Ok(true),
            Err(error)
                if matches!(
                    error.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(WsError::Io(error.to_string())),
        }
    }

    fn next_mask(&mut self) -> [u8; 4] {
        self.mask_state = self
            .mask_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let value = self.mask_state;
        [
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ]
    }
}

/// 掩码种子：时间、进程号与栈地址混合，足够本机链路用。
fn seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0x9E37_79B9_7F4A_7C15);
    let pid = u64::from(std::process::id());
    let local = 0u8;
    let address = std::ptr::from_ref(&local) as usize as u64;
    nanos ^ (pid << 32) ^ address
}

fn mask_bytes(seed: u64) -> [u8; 16] {
    let mut state = seed;
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        chunk.copy_from_slice(&state.to_be_bytes());
    }
    out
}

/// 标准 Base64（带 `=` 填充）。
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        out.push(TABLE[(triple >> 18) as usize & 0x3F] as char);
        out.push(TABLE[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(triple >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[triple as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
