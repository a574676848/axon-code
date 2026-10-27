//! 语音桥：把宿主管道映射为 daemon 侧的会话状态机。
//!
//! 只依赖 [`HostTransport`]，不自己启动进程；进程监督留在宿主 crate 的 supervisor。
//! 上层（daemon 的 `/api/v1/voice`）负责把 [`VoiceEvent`] 映射成 WS 帧。

use std::time::Duration;

use axon_voice_host::supervisor::{HostClient, SupervisorError};
use axon_voice_protocol::{
    Control, ErrorMsg, Final, Frame, Heartbeat, Partial, SessionEnd, SessionStart, Warm, Warmup,
    encode_audio_in,
};

/// 宿主传输层错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// 写失败。
    Write(String),
    /// 读失败。
    Read(String),
    /// 超时。
    Timeout,
    /// 通道关闭。
    Closed,
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Write(reason) => write!(f, "写宿主失败：{reason}"),
            Self::Read(reason) => write!(f, "读宿主失败：{reason}"),
            Self::Timeout => f.write_str("等待宿主超时"),
            Self::Closed => f.write_str("宿主通道已关闭"),
        }
    }
}

/// 宿主传输。
pub trait HostTransport {
    /// 发送控制帧。
    ///
    /// # Errors
    ///
    /// 传输失败。
    fn send_control(&mut self, control: &Control) -> Result<(), TransportError>;
    /// 发送原始帧。
    ///
    /// # Errors
    ///
    /// 传输失败。
    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError>;
    /// 在超时内读取一帧。
    ///
    /// # Errors
    ///
    /// 超时、通道关闭或读取失败。
    fn recv(&mut self, timeout: Duration) -> Result<Frame, TransportError>;
    /// 请求关闭。
    fn close(&mut self);
}

impl HostTransport for HostClient {
    fn send_control(&mut self, control: &Control) -> Result<(), TransportError> {
        self.send(control)
            .map_err(|error: SupervisorError| TransportError::Write(error.to_string()))
    }

    fn send_bytes(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
        self.send_bytes(bytes)
            .map_err(|error: SupervisorError| TransportError::Write(error.to_string()))
    }

    fn recv(&mut self, timeout: Duration) -> Result<Frame, TransportError> {
        self.recv_timeout(timeout).map_err(|error| match error {
            SupervisorError::Timeout => TransportError::Timeout,
            SupervisorError::Read(reason) => TransportError::Read(reason),
            other => TransportError::Read(other.to_string()),
        })
    }

    fn close(&mut self) {
        let _ = self.send(&Control::Close);
    }
}

/// 桥错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeError {
    /// 传输失败。
    Transport(TransportError),
    /// 宿主返回错误帧。
    Host(ErrorMsg),
    /// 状态不允许。
    NotReady,
    /// 已有会话。
    Busy,
    /// 没有会话。
    NoSession,
    /// 协议编码失败。
    Protocol(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(error) => write!(f, "{error}"),
            Self::Host(error) => write!(f, "宿主错误：{}", error.code),
            Self::NotReady => f.write_str("宿主未就绪"),
            Self::Busy => f.write_str("已有会话"),
            Self::NoSession => f.write_str("当前没有会话"),
            Self::Protocol(reason) => write!(f, "协议编码失败：{reason}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// 桥对外事件。
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceEvent {
    /// 预热完成。
    Warm(Warm),
    /// 中间结果。
    Partial(Partial),
    /// 定稿。
    Final(Final),
    /// 对话音频。
    Audio {
        /// 轮次。
        epoch: u32,
        /// 包序号。
        seq: u32,
        /// s16le 载荷。
        audio: Vec<u8>,
    },
    /// cue 音频。
    CueAudio {
        /// cue 标识。
        cue_id: u32,
        /// 包序号。
        seq: u32,
        /// s16le 载荷。
        audio: Vec<u8>,
    },
    /// 宿主错误帧。
    Error(ErrorMsg),
    /// 心跳。
    Heartbeat(Heartbeat),
    /// 关闭确认。
    Closed,
}

/// 桥状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeState {
    /// 未预热。
    Idle,
    /// 已就绪。
    Ready,
    /// 会话中。
    InSession,
    /// 已关闭。
    Closed,
}

/// 语音桥。
pub struct VoiceBridge<T: HostTransport> {
    transport: T,
    state: BridgeState,
    session_id: Option<String>,
}

impl<T: HostTransport> VoiceBridge<T> {
    /// 新建。
    pub fn new(transport: T) -> Self {
        Self {
            transport,
            state: BridgeState::Idle,
            session_id: None,
        }
    }

    /// 当前状态。
    #[must_use]
    pub fn state(&self) -> BridgeState {
        self.state
    }

    /// 预热到就绪。
    ///
    /// # Errors
    ///
    /// 宿主返回错误、超时或传输失败。
    pub fn warmup(
        &mut self,
        family: &str,
        model: &str,
        manifest_sha256: &str,
        timeout: Duration,
    ) -> Result<(), BridgeError> {
        if self.state == BridgeState::Ready {
            return Ok(());
        }
        if self.state != BridgeState::Idle {
            return Err(BridgeError::NotReady);
        }
        self.send_control(&Control::Warmup(Warmup {
            family: family.to_owned(),
            model: model.to_owned(),
            manifest_sha256: manifest_sha256.to_owned(),
        }))?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or(BridgeError::Transport(TransportError::Timeout))?;
            match self.transport.recv(remaining) {
                Ok(Frame::Control(Control::Warm(_))) => {
                    self.state = BridgeState::Ready;
                    return Ok(());
                }
                Ok(Frame::Control(Control::Error(error))) => return Err(BridgeError::Host(error)),
                Ok(Frame::Control(Control::Heartbeat(_))) => continue,
                Ok(_) => continue,
                Err(TransportError::Timeout) => {
                    return Err(BridgeError::Transport(TransportError::Timeout));
                }
                Err(error) => return Err(BridgeError::Transport(error)),
            }
        }
    }

    /// 开始会话。
    ///
    /// # Errors
    ///
    /// 未就绪、已有会话或传输失败。
    pub fn start(
        &mut self,
        session_id: &str,
        mode: &str,
        lang: &str,
        hot_words: &[String],
    ) -> Result<(), BridgeError> {
        if self.state == BridgeState::InSession {
            return Err(BridgeError::Busy);
        }
        if self.state != BridgeState::Ready {
            return Err(BridgeError::NotReady);
        }
        self.send_control(&Control::SessionStart(SessionStart {
            session_id: session_id.to_owned(),
            mode: mode.to_owned(),
            lang: lang.to_owned(),
            hot_words: hot_words.to_vec(),
            sample_rate: 16_000,
        }))?;
        self.state = BridgeState::InSession;
        self.session_id = Some(session_id.to_owned());
        Ok(())
    }

    /// 推入一段 16 kHz 单声道 PCM。
    ///
    /// # Errors
    ///
    /// 没有会话或传输失败。
    pub fn push_audio(&mut self, pcm: &[u8]) -> Result<(), BridgeError> {
        if self.state != BridgeState::InSession {
            return Err(BridgeError::NoSession);
        }
        if pcm.is_empty() {
            return Err(BridgeError::Protocol("音频为空".to_owned()));
        }
        let frame =
            encode_audio_in(pcm).map_err(|error| BridgeError::Protocol(error.to_string()))?;
        self.transport
            .send_bytes(&frame)
            .map_err(BridgeError::Transport)
    }

    /// 取消当前轮。
    ///
    /// # Errors
    ///
    /// 没有会话或传输失败。
    pub fn cancel(&mut self, reason: &str) -> Result<(), BridgeError> {
        if self.state != BridgeState::InSession {
            return Err(BridgeError::NoSession);
        }
        self.send_control(&Control::Cancel(axon_voice_protocol::Cancel {
            reason: reason.to_owned(),
        }))
    }

    /// 结束会话。
    ///
    /// # Errors
    ///
    /// 没有会话或传输失败。
    pub fn end_session(&mut self, reason: &str) -> Result<(), BridgeError> {
        let Some(session_id) = self.session_id.clone() else {
            return Err(BridgeError::NoSession);
        };
        self.send_control(&Control::SessionEnd(SessionEnd {
            session_id,
            reason: reason.to_owned(),
        }))?;
        self.session_id = None;
        self.state = BridgeState::Ready;
        Ok(())
    }

    /// 等待一个事件；超时返回 `Ok(None)`。
    ///
    /// # Errors
    ///
    /// 传输失败。
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<VoiceEvent>, BridgeError> {
        match self.transport.recv(timeout) {
            Ok(frame) => Ok(map_frame(frame)),
            Err(TransportError::Timeout) => Ok(None),
            Err(error) => Err(BridgeError::Transport(error)),
        }
    }

    /// 请求宿主关闭。
    pub fn close(&mut self) {
        self.transport.close();
        self.state = BridgeState::Closed;
    }

    fn send_control(&mut self, control: &Control) -> Result<(), BridgeError> {
        self.transport
            .send_control(control)
            .map_err(BridgeError::Transport)
    }
}

fn map_frame(frame: Frame) -> Option<VoiceEvent> {
    match frame {
        Frame::Control(Control::Warm(warm)) => Some(VoiceEvent::Warm(warm)),
        Frame::Control(Control::Partial(partial)) => Some(VoiceEvent::Partial(partial)),
        Frame::Control(Control::Final(final_message)) => Some(VoiceEvent::Final(final_message)),
        Frame::Control(Control::Heartbeat(beat)) => Some(VoiceEvent::Heartbeat(beat)),
        Frame::Control(Control::Closed) => Some(VoiceEvent::Closed),
        Frame::Control(Control::Error(error)) => Some(VoiceEvent::Error(error)),
        Frame::Control(_) => None,
        Frame::AudioOut { epoch, seq, audio } => Some(VoiceEvent::Audio { epoch, seq, audio }),
        Frame::CueAudio { cue_id, seq, audio } => Some(VoiceEvent::CueAudio { cue_id, seq, audio }),
        Frame::AudioIn(_) => None,
    }
}

#[cfg(test)]
mod tests;
