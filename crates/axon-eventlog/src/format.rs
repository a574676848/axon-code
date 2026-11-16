//! 物理格式：header 行、zstd 帧扫描与编解码。

use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::error::EventLogError;
use crate::FORMAT_VERSION;

/// header 固定占用第 0 帧。
pub const HEADER_FRAME_INDEX: usize = 0;
/// zstd 帧魔数（小端）。
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// 压缩级别。与现有归档一致，取 3。
const COMPRESSION_LEVEL: i32 = 3;
/// 单帧解压明文上限，避免损坏数据触发无界分配。
const MAX_FRAME_PLAIN_BYTES: usize = 64 * 1024 * 1024;

/// 会话日志头部，放在第 0 帧。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    /// 物理格式代数。
    pub format_version: u32,
    /// 会话标识。
    pub session_id: String,
    /// 创建时刻（Unix 毫秒）。
    pub created_at_ms: u64,
    /// 创建时的工作目录。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 是否为继承会话。
    #[serde(default)]
    pub seed: bool,
}

impl Header {
    /// 构造当前代数的头部。
    #[must_use]
    pub fn new(session_id: impl Into<String>, created_at_ms: u64, cwd: Option<String>) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            session_id: session_id.into(),
            created_at_ms,
            cwd,
            seed: false,
        }
    }

    /// 编码成一行 JSON（含换行）。
    pub fn encode_line(&self) -> Result<Vec<u8>, EventLogError> {
        let mut line =
            serde_json::to_vec(self).map_err(|error| EventLogError::Codec(error.to_string()))?;
        line.push(b'\n');
        Ok(line)
    }

    /// 从一行 JSON 解码并校验版本。
    pub fn decode_line(line: &[u8]) -> Result<Self, EventLogError> {
        let header: Self = serde_json::from_slice(trim_line(line))
            .map_err(|error| EventLogError::Codec(error.to_string()))?;
        if header.format_version != FORMAT_VERSION {
            return Err(EventLogError::UnsupportedVersion(header.format_version));
        }
        Ok(header)
    }
}

fn trim_line(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && matches!(line[end - 1], b'\n' | b'\r') {
        end -= 1;
    }
    &line[..end]
}

/// 一个完整帧在字节流中的起止位置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSpan {
    /// 帧起始字节。
    pub start: usize,
    /// 帧结束字节（不含）。
    pub end: usize,
}

/// 帧扫描结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameScan {
    /// 已完整识别的帧。
    pub frames: Vec<FrameSpan>,
    /// 末尾不完整帧的起始位置；没有则为 `None`。
    pub torn_start: Option<usize>,
}

/// 按 zstd 帧结构扫描字节流，不解压。
///
/// 规则来自 RFC 8878：魔数、帧头描述符、可选字段、块循环、可选校验和。
/// 末尾不足一个完整帧时返回 `torn_start`，交由写句柄截断。
pub fn scan_frames(bytes: &[u8]) -> Result<FrameScan, EventLogError> {
    let mut frames = Vec::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let start = cursor;
        if bytes.len() - cursor < 4 {
            return Ok(FrameScan {
                frames,
                torn_start: Some(start),
            });
        }
        if bytes[cursor..cursor + 4] != ZSTD_MAGIC {
            return Err(EventLogError::Corrupt(format!("帧魔数无效，偏移 {cursor}")));
        }
        cursor += 4;

        let Some(&descriptor) = bytes.get(cursor) else {
            return Ok(torn_scan(frames, start));
        };
        cursor += 1;
        if descriptor & 0b0001_1000 != 0 {
            return Err(EventLogError::Corrupt(format!("帧头保留位非零，偏移 {start}")));
        }
        let content_size_flag = descriptor >> 6;
        let single_segment = descriptor & 0b0010_0000 != 0;
        let checksum_flag = descriptor & 0b0000_0100 != 0;
        let dictionary_flag = descriptor & 0b0000_0011;
        let dictionary_bytes = if dictionary_flag == 3 { 4 } else { usize::from(dictionary_flag) };
        let content_size_bytes = if content_size_flag == 0 {
            usize::from(single_segment)
        } else {
            1usize << content_size_flag
        };
        let skip = dictionary_bytes + content_size_bytes + usize::from(!single_segment);
        if bytes.len() - cursor < skip {
            return Ok(torn_scan(frames, start));
        }
        cursor += skip;

        loop {
            if bytes.len() - cursor < 3 {
                return Ok(torn_scan(frames, start));
            }
            let header = u32::from(bytes[cursor])
                | (u32::from(bytes[cursor + 1]) << 8)
                | (u32::from(bytes[cursor + 2]) << 16);
            cursor += 3;
            let last_block = header & 1 == 1;
            let block_type = (header >> 1) & 0b11;
            let block_size = (header >> 3) as usize;
            if block_type == 0b11 {
                return Err(EventLogError::Corrupt(format!("保留块类型，偏移 {cursor}")));
            }
            let payload = if block_type == 0b01 { 1 } else { block_size };
            if bytes.len() - cursor < payload {
                return Ok(torn_scan(frames, start));
            }
            cursor += payload;
            if last_block {
                break;
            }
        }

        if checksum_flag {
            if bytes.len() - cursor < 4 {
                return Ok(torn_scan(frames, start));
            }
            cursor += 4;
        }
        frames.push(FrameSpan {
            start,
            end: cursor,
        });
    }
    Ok(FrameScan {
        frames,
        torn_start: None,
    })
}

fn torn_scan(frames: Vec<FrameSpan>, start: usize) -> FrameScan {
    FrameScan {
        frames,
        torn_start: Some(start),
    }
}

/// 压缩一段明文成一个带校验和的 zstd 帧。
pub fn encode_frame(plain: &[u8]) -> Result<Vec<u8>, EventLogError> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), COMPRESSION_LEVEL)
        .map_err(|error| EventLogError::Codec(error.to_string()))?;
    encoder
        .include_checksum(true)
        .map_err(|error| EventLogError::Codec(error.to_string()))?;
    encoder.write_all(plain)?;
    encoder
        .finish()
        .map_err(|error| EventLogError::Codec(error.to_string()))
}

/// 解压单个完整帧。
pub fn decode_frame(frame: &[u8]) -> Result<Vec<u8>, EventLogError> {
    let plain = zstd::decode_all(frame).map_err(|error| {
        EventLogError::Corrupt(format!("zstd 帧解码失败：{error}"))
    })?;
    if plain.len() > MAX_FRAME_PLAIN_BYTES {
        return Err(EventLogError::Corrupt(format!(
            "帧明文超过上限：{} 字节",
            plain.len()
        )));
    }
    Ok(plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips_and_scan_finds_it() {
        let first = encode_frame(b"frame-one\n").expect("可压缩");
        let second = encode_frame(b"frame-two\n").expect("可压缩");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&first);
        bytes.extend_from_slice(&second);

        let scan = scan_frames(&bytes).expect("可扫描");
        assert_eq!(scan.frames.len(), 2);
        assert_eq!(scan.torn_start, None);
        assert_eq!(&bytes[scan.frames[0].start..scan.frames[0].end], first.as_slice());
        assert_eq!(&bytes[scan.frames[1].start..scan.frames[1].end], second.as_slice());
    }

    #[test]
    fn truncated_tail_is_reported_as_torn() {
        let frame = encode_frame(&vec![b'x'; 4096]).expect("可压缩");
        let cut = frame.len() - 8;
        let scan = scan_frames(&frame[..cut]).expect("可扫描");
        assert!(scan.frames.is_empty(), "不完整帧不应被识别");
        assert_eq!(scan.torn_start, Some(0));
    }

    #[test]
    fn invalid_magic_is_corruption() {
        let error = scan_frames(b"not a zstd frame").expect_err("应拒绝");
        assert!(matches!(error, EventLogError::Corrupt(_)));
    }

    #[test]
    fn header_rejects_foreign_version() {
        let line = br#"{"formatVersion":99,"sessionId":"s","createdAtMs":1,"seed":false}"#;
        let error = Header::decode_line(line).expect_err("应拒绝外来版本");
        assert!(matches!(error, EventLogError::UnsupportedVersion(99)));
    }
}
