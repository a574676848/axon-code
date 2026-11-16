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
    for span in scan.frames.iter().skip(HEADER_FRAME_INDEX + 1) {
        let plain = decode_frame(&bytes[span.start..span.end])?;
        // 起始序号由控制面决定，这里只校验相邻连续，不假定从 0 开始。
        let mut expected: Option<u64> = None;
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
