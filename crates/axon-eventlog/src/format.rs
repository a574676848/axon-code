//! 物理格式：header 行、zstd 帧扫描与编解码。

use std::io::Write as _;

use serde::{Deserialize, Serialize};

use crate::FORMAT_VERSION;
use crate::error::EventLogError;

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

/// 帧扫描的字节来源。
///
/// 扫描规则只写一遍，内存切片与磁盘文件各给一份读取实现：读一页分页日志时
/// 不必把整个文件搬进内存，而扫描判据与写侧的截断判据仍然是同一份代码。
pub trait FrameSource {
    /// 来源的总字节数。
    fn total_len(&mut self) -> Result<usize, EventLogError>;

    /// 读取从 `start` 起的至多 `len` 字节；末尾不足时返回实际读到的字节。
    fn read_range(&mut self, start: usize, len: usize) -> Result<Vec<u8>, EventLogError>;
}

impl FrameSource for &[u8] {
    fn total_len(&mut self) -> Result<usize, EventLogError> {
        Ok(self.len())
    }

    fn read_range(&mut self, start: usize, len: usize) -> Result<Vec<u8>, EventLogError> {
        let end = start.saturating_add(len).min(self.len());
        Ok(self.get(start..end).unwrap_or_default().to_vec())
    }
}

impl FrameSource for std::fs::File {
    fn total_len(&mut self) -> Result<usize, EventLogError> {
        let len = self.metadata()?.len();
        usize::try_from(len)
            .map_err(|_| EventLogError::Corrupt(format!("日志长度超出可寻址范围：{len} 字节")))
    }

    fn read_range(&mut self, start: usize, len: usize) -> Result<Vec<u8>, EventLogError> {
        use std::io::{Read as _, Seek as _, SeekFrom};
        self.seek(SeekFrom::Start(u64::try_from(start).unwrap_or(u64::MAX)))?;
        let mut buffer = vec![0u8; len];
        let mut filled = 0usize;
        while filled < len {
            let read = self.read(&mut buffer[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        buffer.truncate(filled);
        Ok(buffer)
    }
}

/// 按 zstd 帧结构扫描字节流，不解压。
///
/// 规则来自 RFC 8878：魔数、帧头描述符、可选字段、块循环、可选校验和。
/// 末尾不足一个完整帧时返回 `torn_start`，交由写句柄截断。
pub fn scan_frames(bytes: &[u8]) -> Result<FrameScan, EventLogError> {
    let mut source = bytes;
    scan_source_frames(&mut source)
}

/// 按帧结构扫描磁盘文件：只读帧头与块头，不把整个文件读进内存。
pub fn scan_file_frames(file: &mut std::fs::File) -> Result<FrameScan, EventLogError> {
    scan_source_frames(file)
}

/// 扫描任意字节来源，判据与 [`scan_frames`] 完全一致。
pub fn scan_source_frames<S: FrameSource>(source: &mut S) -> Result<FrameScan, EventLogError> {
    let total = source.total_len()?;
    let mut frames = Vec::new();
    let mut cursor = 0usize;
    while cursor < total {
        let Some(span) = step_frame(source, cursor, total)? else {
            return Ok(torn_scan(frames, cursor));
        };
        cursor = span.end;
        frames.push(span);
    }
    Ok(FrameScan {
        frames,
        torn_start: None,
    })
}

/// 从 `start` 起解析一帧，返回它的字节区间；尾段不足一帧时返回 `None`。
///
/// 索引把起点帧定位到字节偏移后，读路径靠它逐帧前进：只解析需要的那几帧，
/// 不必先把整条日志的帧结构扫一遍。
pub(crate) fn step_frame<S: FrameSource>(
    source: &mut S,
    start: usize,
    total: usize,
) -> Result<Option<FrameSpan>, EventLogError> {
    let mut cursor = start;
    let magic = source.read_range(cursor, ZSTD_MAGIC.len())?;
    if magic.len() < ZSTD_MAGIC.len() {
        return Ok(None);
    }
    if magic != ZSTD_MAGIC {
        return Err(EventLogError::Corrupt(format!("帧魔数无效，偏移 {cursor}")));
    }
    cursor += ZSTD_MAGIC.len();

    let descriptor_bytes = source.read_range(cursor, 1)?;
    let Some(&descriptor) = descriptor_bytes.first() else {
        return Ok(None);
    };
    cursor += 1;
    if descriptor & 0b0001_1000 != 0 {
        return Err(EventLogError::Corrupt(format!(
            "帧头保留位非零，偏移 {start}"
        )));
    }
    let content_size_flag = descriptor >> 6;
    let single_segment = descriptor & 0b0010_0000 != 0;
    let checksum_flag = descriptor & 0b0000_0100 != 0;
    let dictionary_flag = descriptor & 0b0000_0011;
    let dictionary_bytes = if dictionary_flag == 3 {
        4
    } else {
        usize::from(dictionary_flag)
    };
    let content_size_bytes = if content_size_flag == 0 {
        usize::from(single_segment)
    } else {
        1usize << content_size_flag
    };
    let skip = dictionary_bytes + content_size_bytes + usize::from(!single_segment);
    if total - cursor < skip {
        return Ok(None);
    }
    cursor += skip;

    loop {
        if total - cursor < 3 {
            return Ok(None);
        }
        let block_header = source.read_range(cursor, 3)?;
        let [first, second, third] = match block_header.as_slice() {
            [first, second, third] => [*first, *second, *third],
            _ => return Ok(None),
        };
        let header = u32::from(first) | (u32::from(second) << 8) | (u32::from(third) << 16);
        cursor += 3;
        let last_block = header & 1 == 1;
        let block_type = (header >> 1) & 0b11;
        let block_size = (header >> 3) as usize;
        if block_type == 0b11 {
            return Err(EventLogError::Corrupt(format!("保留块类型，偏移 {cursor}")));
        }
        let payload = if block_type == 0b01 { 1 } else { block_size };
        if total - cursor < payload {
            return Ok(None);
        }
        cursor += payload;
        if last_block {
            break;
        }
    }

    if checksum_flag {
        if total - cursor < 4 {
            return Ok(None);
        }
        cursor += 4;
    }
    Ok(Some(FrameSpan { start, end: cursor }))
}

fn torn_scan(frames: Vec<FrameSpan>, start: usize) -> FrameScan {
    FrameScan {
        frames,
        torn_start: Some(start),
    }
}

/// 尾部字节是否恰好停在一个完整帧边界上。
///
/// 判据是「尾段里某个 zstd 帧魔数起的剩余字节能被完整解码」：帧自带内容校验和，
/// 随机字节几乎不可能凑出可解码的尾帧，因此误判概率可忽略。
/// 返回 `false` 只说明「尾段里找不到完整帧」，调用方应回退全量扫描定位截断点。
pub(crate) fn tail_ends_on_frame_boundary(tail: &[u8]) -> bool {
    if tail.is_empty() {
        return true;
    }
    tail.windows(ZSTD_MAGIC.len())
        .enumerate()
        .any(|(offset, window)| window == ZSTD_MAGIC && decode_frame(&tail[offset..]).is_ok())
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
    let plain = zstd::decode_all(frame)
        .map_err(|error| EventLogError::Corrupt(format!("zstd 帧解码失败：{error}")))?;
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
        assert_eq!(
            &bytes[scan.frames[0].start..scan.frames[0].end],
            first.as_slice()
        );
        assert_eq!(
            &bytes[scan.frames[1].start..scan.frames[1].end],
            second.as_slice()
        );
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
