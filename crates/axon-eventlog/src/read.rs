//! 只读快照与查询。

use std::path::Path;

use crate::error::EventLogError;
use crate::format::{
    FrameSpan, HEADER_FRAME_INDEX, Header, decode_frame, scan_file_frames, scan_frames, step_frame,
};
use crate::index;
use crate::record::RuntimeEventRecord;

/// 一次完整读取的结果。
#[derive(Debug, Clone, PartialEq)]
pub struct EventLogSnapshot {
    /// 会话头部。
    pub header: Header,
    /// 按序号升序的全部事件。
    pub records: Vec<RuntimeEventRecord>,
    /// 末尾不完整帧的起始字节；`Some` 表示日志尾部被截断。
    pub torn_truncate_to: Option<usize>,
}

/// 读取指定路径的完整快照。
///
/// 完整帧内出现半条记录、或中途解压失败，都按损坏处理并拒绝，不猜测内容。
pub fn read_snapshot(path: &Path) -> Result<EventLogSnapshot, EventLogError> {
    let bytes = std::fs::read(path)?;
    let scan = scan_frames(&bytes)?;
    let header_span = scan
        .frames
        .get(HEADER_FRAME_INDEX)
        .ok_or_else(|| EventLogError::Corrupt("日志缺少 header 帧".to_owned()))?;
    let header_plain = decode_frame(&bytes[header_span.start..header_span.end])?;
    let newlines = header_plain.iter().filter(|byte| **byte == b'\n').count();
    if newlines != 1 {
        return Err(EventLogError::Corrupt(format!(
            "header 帧必须恰好包含一行，实际换行数 {newlines}"
        )));
    }
    let header = Header::decode_line(&header_plain)?;

    let mut records = Vec::new();
    // 跨帧同样要求序号连续：只校验帧内连续会让「控制面水位倒退后写出的重复序号」
    // 静默通过，读者拿到的序号序列不再单调。
    let mut expected: Option<u64> = None;
    for span in scan.frames.iter().skip(HEADER_FRAME_INDEX + 1) {
        let plain = decode_frame(&bytes[span.start..span.end])?;
        for record in decode_records(&plain)? {
            if let Some(previous) = expected
                && record.sequence != previous
            {
                return Err(EventLogError::SequenceGap {
                    expected: previous,
                    actual: record.sequence,
                });
            }
            expected = Some(record.sequence.saturating_add(1));
            records.push(record);
        }
    }

    Ok(EventLogSnapshot {
        header,
        records,
        torn_truncate_to: scan.torn_start,
    })
}

/// 事件日志的只读读者。
#[derive(Debug, Clone, PartialEq)]
pub struct EventLogReader {
    snapshot: EventLogSnapshot,
}
/// 读取序号严格大于 `after` 的至多 `limit` 条事件。
///
/// 有索引时二分到起点帧后只解压需要的帧；没有索引（冷启、旧日志、索引落后）时从
/// 首帧单趟顺序解压并跳过起点之前的部分。两条路径都是单趟，不会为翻一页把整条
/// 日志重复解压。
pub fn read_after(
    path: &Path,
    after: u64,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let start = index::load(path, len)
        .and_then(|index| index.locate(after))
        .map_or(0, |entry| entry.offset);
    collect_after(&mut file, start, len, after, limit)
}

/// 从 `start` 起顺序读取并收集序号大于 `after` 的事件。
fn collect_after(
    file: &mut std::fs::File,
    start: u64,
    total: u64,
    after: u64,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    let total = usize::try_from(total).unwrap_or(usize::MAX);
    let mut cursor = usize::try_from(start).unwrap_or(0);
    // 从文件起点开始时，第一帧是 header 帧：它只有一行 header，不是事件记录。
    let mut skip_header = cursor == 0;
    // 上限只为挡住调用方传入的极端值，不预分配 `limit` 条（可能是 usize::MAX）。
    let mut collected: Vec<RuntimeEventRecord> = Vec::with_capacity(limit.min(1024));
    while cursor < total {
        let Some(span) = step_frame(file, cursor, total)? else {
            break;
        };
        cursor = span.end;
        if skip_header {
            skip_header = false;
            continue;
        }
        for record in frame_records(file, &span)? {
            if record.sequence > after {
                collected.push(record);
                if collected.len() >= limit {
                    return Ok(collected);
                }
            }
        }
    }
    Ok(collected)
}

/// 日志文件长度；文件不存在时返回 `None`。
fn log_len(path: &Path) -> Result<Option<u64>, EventLogError> {
    match std::fs::metadata(path) {
        Ok(meta) => Ok(Some(meta.len())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// 从尾部倒推读取满足谓词的最后 `limit` 条事件，只解压必要的尾部帧。
pub fn read_tail_matching<F>(
    path: &Path,
    limit: usize,
    predicate: F,
) -> Result<Vec<RuntimeEventRecord>, EventLogError>
where
    F: Fn(&RuntimeEventRecord) -> bool,
{
    if limit == 0 {
        return Ok(Vec::new());
    }
    let Some(len) = log_len(path)? else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(path)?;
    let mut matched: Vec<RuntimeEventRecord> = Vec::new();
    // 有索引时按条目倒着解压，连帧结构扫描都省掉。
    if let Some(index) = index::load(path, len) {
        for entry in index.entries().iter().rev() {
            if matched.len() >= limit {
                break;
            }
            let (Ok(start), Ok(end)) =
                (usize::try_from(entry.offset), usize::try_from(entry.end()))
            else {
                break;
            };
            let span = FrameSpan { start, end };
            push_matches(
                &mut matched,
                frame_records(&mut file, &span)?,
                &predicate,
                limit,
            );
        }
        return Ok(matched);
    }
    let scan = scan_file_frames(&mut file)?;
    let frames: Vec<FrameSpan> = scan
        .frames
        .iter()
        .skip(HEADER_FRAME_INDEX + 1)
        .copied()
        .collect();
    for span in frames.iter().rev() {
        if matched.len() >= limit {
            break;
        }
        push_matches(
            &mut matched,
            frame_records(&mut file, span)?,
            &predicate,
            limit,
        );
    }
    Ok(matched)
}

/// 把一帧里命中谓词的事件按原序并入 `matched`，保持尾部窗口语义。
fn push_matches<F>(
    matched: &mut Vec<RuntimeEventRecord>,
    records: Vec<RuntimeEventRecord>,
    predicate: &F,
    limit: usize,
) where
    F: Fn(&RuntimeEventRecord) -> bool,
{
    let hits: Vec<RuntimeEventRecord> = records
        .into_iter()
        .filter(|record| predicate(record))
        .collect();
    for record in hits.into_iter().rev() {
        matched.insert(0, record);
    }
    if matched.len() > limit {
        matched.drain(..matched.len() - limit);
    }
}

/// 读取落盘末条事件；日志不存在或没有完整帧时返回 `None`。
///
/// 会话恢复与写入补偿只要这一条事实：有索引时只解压末帧，不必为拿尾记录把整条
/// 日志解一遍。
pub fn read_tail_record(path: &Path) -> Result<Option<RuntimeEventRecord>, EventLogError> {
    let Some(len) = log_len(path)? else {
        return Ok(None);
    };
    let mut file = std::fs::File::open(path)?;
    if let Some(entry) = index::load(path, len).and_then(|index| index.last()) {
        let (Ok(start), Ok(end)) = (usize::try_from(entry.offset), usize::try_from(entry.end()))
        else {
            return Ok(None);
        };
        let span = FrameSpan { start, end };
        return Ok(frame_records(&mut file, &span)?.pop());
    }
    let scan = scan_file_frames(&mut file)?;
    let Some(span) = scan.frames.iter().skip(HEADER_FRAME_INDEX + 1).next_back() else {
        return Ok(None);
    };
    Ok(frame_records(&mut file, span)?.pop())
}

/// 解出 header 帧：必须恰好一行，且版本受支持。
pub(crate) fn decode_header_frame(bytes: &[u8]) -> Option<Header> {
    let plain = decode_frame(bytes).ok()?;
    let newlines = plain.iter().filter(|byte| **byte == b'\n').count();
    if newlines != 1 {
        return None;
    }
    Header::decode_line(&plain).ok()
}

/// 读出并解压单帧：只把这一帧的字节搬进内存。
pub(crate) fn frame_records(
    file: &mut std::fs::File,
    span: &FrameSpan,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let len = span.end.saturating_sub(span.start);
    file.seek(SeekFrom::Start(
        u64::try_from(span.start).unwrap_or(u64::MAX),
    ))?;
    let mut buffer = vec![0u8; len];
    file.read_exact(&mut buffer)?;
    decode_records(&decode_frame(&buffer)?)
}

/// 把一帧明文拆成事件记录，并校验帧内序号连续。
fn decode_records(plain: &[u8]) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    // 起始序号由控制面决定，这里只校验相邻连续，不假定从 0 开始。
    let mut expected: Option<u64> = None;
    let mut records = Vec::new();
    for line in plain.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let record = RuntimeEventRecord::decode_line(line)?;
        if let Some(previous) = expected
            && record.sequence != previous
        {
            return Err(EventLogError::SequenceGap {
                expected: previous,
                actual: record.sequence,
            });
        }
        expected = Some(record.sequence.saturating_add(1));
        records.push(record);
    }
    Ok(records)
}

impl EventLogReader {
    /// 打开并解析一个日志文件。
    pub fn open(path: &Path) -> Result<Self, EventLogError> {
        Ok(Self {
            snapshot: read_snapshot(path)?,
        })
    }

    /// 用已有快照构造读者。
    #[must_use]
    pub fn from_snapshot(snapshot: EventLogSnapshot) -> Self {
        Self { snapshot }
    }

    /// 会话头部。
    #[must_use]
    pub fn header(&self) -> &Header {
        &self.snapshot.header
    }

    /// 全部事件。
    #[must_use]
    pub fn records(&self) -> &[RuntimeEventRecord] {
        &self.snapshot.records
    }

    /// 末条事件的序号。
    #[must_use]
    pub fn last_sequence(&self) -> Option<u64> {
        self.snapshot.records.last().map(|record| record.sequence)
    }

    /// 读取序号严格大于 `after` 的至多 `limit` 条事件。
    #[must_use]
    pub fn read_from(&self, after: u64, limit: usize) -> Vec<RuntimeEventRecord> {
        self.snapshot
            .records
            .iter()
            .filter(|record| record.sequence > after)
            .take(limit)
            .cloned()
            .collect()
    }

    /// 读取尾部至多 `limit` 条事件，按序号升序返回。
    #[must_use]
    pub fn read_tail(&self, limit: usize) -> Vec<RuntimeEventRecord> {
        let total = self.snapshot.records.len();
        let start = total.saturating_sub(limit);
        self.snapshot.records[start..].to_vec()
    }

    /// 按谓词过滤读取，用于按 interaction 定向取帧。
    #[must_use]
    pub fn read_matching<F>(&self, limit: usize, predicate: F) -> Vec<RuntimeEventRecord>
    where
        F: Fn(&RuntimeEventRecord) -> bool,
    {
        self.snapshot
            .records
            .iter()
            .filter(|record| predicate(record))
            .take(limit)
            .cloned()
            .collect()
    }
}
