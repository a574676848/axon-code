//! 帧：类型、编解码、读写与音频载荷。

use std::io::{self, Read, Write};

use crate::control::Control;
use crate::{KIND_AUDIO_IN, KIND_AUDIO_OUT, KIND_CONTROL, KIND_CUE_AUDIO, MAX_FRAME_BYTES};

/// 一帧。
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// 控制帧。
    Control(Control),
    /// 上行音频，裸 s16le。
    AudioIn(Vec<u8>),
    /// 对话音频。
    AudioOut {
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
}

/// 编解码错误。
#[derive(Debug)]
pub enum FrameError {
    /// IO 失败。
    Io(io::Error),
    /// 音频载荷不是偶数长度。
    OddAudioLength,
    /// 帧长度超过上限。
    TooLarge {
        /// 实际长度。
        len: usize,
    },
    /// 载荷不足（截断）。
    Truncated,
    /// 未知帧类型。
    UnknownKind(u8),
    /// JSON 非法。
    InvalidJson(serde_json::Error),
    /// 消息类型未知。
    UnknownType(String),
    /// 序列化失败。
    Serialize(serde_json::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io: {error}"),
            Self::OddAudioLength => f.write_str("音频载荷长度必须为偶数"),
            Self::TooLarge { len } => write!(f, "帧长度 {len} 超过上限"),
            Self::Truncated => f.write_str("帧被截断"),
            Self::UnknownKind(kind) => write!(f, "未知帧类型 {kind:#04x}"),
            Self::InvalidJson(error) => write!(f, "json 非法：{error}"),
            Self::UnknownType(kind) => write!(f, "未知消息类型：{kind}"),
            Self::Serialize(error) => write!(f, "序列化失败：{error}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 解析完整帧（含头）。
///
/// # Errors
///
/// 长度、类型或载荷非法。
pub fn decode_frame(bytes: &[u8]) -> Result<Frame, FrameError> {
    let Some((&kind, rest)) = bytes.split_first() else {
        return Err(FrameError::Truncated);
    };
    if rest.len() < 4 {
        return Err(FrameError::Truncated);
    }
    let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len });
    }
    let payload = rest.get(4..4 + len).ok_or(FrameError::Truncated)?;
    decode_kind(kind, payload)
}

fn decode_kind(kind: u8, payload: &[u8]) -> Result<Frame, FrameError> {
    match kind {
        KIND_CONTROL => Ok(Frame::Control(Control::decode(payload)?)),
        KIND_AUDIO_IN => {
            ensure_even(payload)?;
            Ok(Frame::AudioIn(payload.to_vec()))
        }
        KIND_AUDIO_OUT => {
            let (epoch, seq, audio) = split_audio(payload)?;
            Ok(Frame::AudioOut { epoch, seq, audio })
        }
        KIND_CUE_AUDIO => {
            let (cue_id, seq, audio) = split_audio(payload)?;
            Ok(Frame::CueAudio { cue_id, seq, audio })
        }
        other => Err(FrameError::UnknownKind(other)),
    }
}

fn split_audio(payload: &[u8]) -> Result<(u32, u32, Vec<u8>), FrameError> {
    if payload.len() < 8 {
        return Err(FrameError::Truncated);
    }
    let first = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let second = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let audio = payload[8..].to_vec();
    ensure_even(&audio)?;
    Ok((first, second, audio))
}

fn ensure_even(payload: &[u8]) -> Result<(), FrameError> {
    if payload.len().is_multiple_of(2) {
        Ok(())
    } else {
        Err(FrameError::OddAudioLength)
    }
}

/// 编码上行音频帧。
///
/// # Errors
///
/// 载荷为奇数长度或超过上限。
pub fn encode_audio_in(samples: &[u8]) -> Result<Vec<u8>, FrameError> {
    encode_audio(KIND_AUDIO_IN, &[], samples)
}

/// 编码对话音频帧。
///
/// # Errors
///
/// 载荷为奇数长度或超过上限。
pub fn encode_audio_out(epoch: u32, seq: u32, samples: &[u8]) -> Result<Vec<u8>, FrameError> {
    let header = [epoch.to_be_bytes(), seq.to_be_bytes()].concat();
    encode_audio(KIND_AUDIO_OUT, &header, samples)
}

/// 编码 cue 音频帧。
///
/// # Errors
///
/// 载荷为奇数长度或超过上限。
pub fn encode_cue_audio(cue_id: u32, seq: u32, samples: &[u8]) -> Result<Vec<u8>, FrameError> {
    let header = [cue_id.to_be_bytes(), seq.to_be_bytes()].concat();
    encode_audio(KIND_CUE_AUDIO, &header, samples)
}

fn encode_audio(kind: u8, header: &[u8], samples: &[u8]) -> Result<Vec<u8>, FrameError> {
    ensure_even(samples)?;
    let len = header.len() + samples.len();
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len });
    }
    let mut frame = Vec::with_capacity(len + 5);
    frame.push(kind);
    frame.extend_from_slice(&(len as u32).to_be_bytes());
    frame.extend_from_slice(header);
    frame.extend_from_slice(samples);
    Ok(frame)
}

/// 从流里读一帧；对端正常关闭时返回 `Ok(None)`。
///
/// # Errors
///
/// IO 失败、帧超限或载荷非法。
pub fn read_frame(reader: &mut impl Read) -> Result<Option<Frame>, FrameError> {
    let mut kind = [0u8; 1];
    match reader.read_exact(&mut kind) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(FrameError::Io(error)),
    }
    let mut header = [0u8; 4];
    reader.read_exact(&mut header)?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(FrameError::TooLarge { len });
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    decode_kind(kind[0], &payload).map(Some)
}

/// 写一帧并刷新。
///
/// # Errors
///
/// 序列化或 IO 失败。
pub fn write_frame(writer: &mut impl Write, frame: &Control) -> Result<(), FrameError> {
    let bytes = frame.encode()?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

/// 写原始帧字节并刷新。
///
/// # Errors
///
/// IO 失败。
pub fn write_bytes(writer: &mut impl Write, bytes: &[u8]) -> Result<(), FrameError> {
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Frame, FrameError, decode_frame, encode_audio_out, read_frame};
    use std::io::Cursor;

    #[test]
    fn oversized_length_header_is_rejected() {
        let mut bytes = vec![crate::KIND_CONTROL];
        bytes.extend_from_slice(&((crate::MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        assert!(matches!(
            decode_frame(&bytes),
            Err(FrameError::TooLarge { .. })
        ));
    }

    #[test]
    fn truncated_frame_is_rejected() {
        assert!(matches!(decode_frame(&[]), Err(FrameError::Truncated)));
        assert!(matches!(
            decode_frame(&[crate::KIND_CONTROL, 0, 0]),
            Err(FrameError::Truncated)
        ));
    }

    #[test]
    fn audio_frames_are_parsed_with_headers() {
        let frame = encode_audio_out(7, 9, &[0, 1, 2, 3]).expect("编码应成功");
        match decode_frame(&frame).expect("应解析") {
            Frame::AudioOut { epoch, seq, audio } => {
                assert_eq!((epoch, seq), (7, 9));
                assert_eq!(audio, vec![0, 1, 2, 3]);
            }
            other => panic!("期望音频帧，得到 {other:?}"),
        }
    }

    #[test]
    fn odd_audio_length_is_rejected() {
        let mut bytes = vec![crate::KIND_AUDIO_IN];
        bytes.extend_from_slice(&3u32.to_be_bytes());
        bytes.extend_from_slice(&[1, 2, 3]);
        assert!(matches!(
            decode_frame(&bytes),
            Err(FrameError::OddAudioLength)
        ));
    }

    #[test]
    fn unknown_kind_is_rejected() {
        let mut bytes = vec![0x7fu8];
        bytes.extend_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            decode_frame(&bytes),
            Err(FrameError::UnknownKind(0x7f))
        ));
    }

    #[test]
    fn read_frame_returns_none_on_eof() {
        let mut cursor = Cursor::new(Vec::new());
        assert!(read_frame(&mut cursor).expect("EOF 应成功").is_none());
    }
}
