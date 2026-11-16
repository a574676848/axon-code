//! 只读快照与查询。

use std::path::Path;

use crate::error::EventLogError;
use crate::format::{HEADER_FRAME_INDEX, Header, decode_frame, scan_frames};
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
/// 读取序号严格大于 `after` 的至多 `limit` 条事件，只解压必要的尾部帧。
///
/// 与 [`read_snapshot`] 的区别是**不做全量解压**：帧结构扫描仍然要读一遍文件字节，
/// 但只解压从尾部倒推覆盖 `after` 的那几帧。日志越大，收益越明显；分页 replay
/// 每翻一页都从尾部重新定位，不会随历史增长而线性变慢。
pub fn read_after(
    path: &Path,
    after: u64,
    limit: usize,
) -> Result<Vec<RuntimeEventRecord>, EventLogError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let bytes = std::fs::read(path)?;
    let scan = scan_frames(&bytes)?;
    let frames: Vec<_> = scan
        .frames
        .iter()
        .skip(HEADER_FRAME_INDEX + 1)
        .copied()
        .collect();
    // 先把最后一帧的末条序号作为快路径：`after` 已越过全部记录时直接返回空。
    if let Some(last_span) = frames.last() {
        let plain = decode_frame(&bytes[last_span.start..last_span.end])?;
        if decode_records(&plain)?
            .last()
            .is_none_or(|record| record.sequence <= after)
        {
            return Ok(Vec::new());
        }
    }
    // 再从尾部倒推定位「包含 after 的那一帧」，从那一帧往后顺序解压；
    // 没有任何一帧的第一条序号 <= after 时，说明 after 落在日志起点之前，从头开始。
    let mut begin = 0usize;
    for (index, span) in frames.iter().enumerate().rev() {
        let plain = decode_frame(&bytes[span.start..span.end])?;
        let first = decode_records(&plain)?
            .first()
            .map(|record| record.sequence);
        if first.is_none_or(|sequence| sequence <= after) {
            begin = index;
            break;
        }
    }
    let mut collected: Vec<RuntimeEventRecord> = Vec::with_capacity(limit);
    for span in frames.iter().skip(begin) {
        let plain = decode_frame(&bytes[span.start..span.end])?;
        for record in decode_records(&plain)? {
            if record.sequence > after {
                collected.push(record);
            }
            if collected.len() >= limit {
                return Ok(collected);
            }
        }
    }
    Ok(collected)
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
    let bytes = std::fs::read(path)?;
    let scan = scan_frames(&bytes)?;
    let mut matched: Vec<RuntimeEventRecord> = Vec::new();
    for span in scan.frames.iter().skip(HEADER_FRAME_INDEX + 1).rev() {
        if matched.len() >= limit {
            break;
        }
        let plain = decode_frame(&bytes[span.start..span.end])?;
        let records = decode_records(&plain)?;
        let tail: Vec<RuntimeEventRecord> = records
            .into_iter()
            .filter(|record| predicate(record))
            .collect();
        for record in tail.into_iter().rev() {
            matched.insert(0, record);
        }
    }
    if matched.len() > limit {
        matched.drain(..matched.len() - limit);
    }
    Ok(matched)
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
