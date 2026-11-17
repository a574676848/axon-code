//! 写句柄侧的索引维护。
//!
//! 与日志写入分开成文：追加路径只关心"帧落盘 + 条目追加"，而这里关心的是
//! 「打开既有日志时怎么用索引免掉全量解压」和「索引落后时怎么重建」。
//!
//! 三条路径的取舍：
//! - **快路径**：覆盖长度与日志等长 → 只解压 header 帧与末帧，代价与日志长度无关；
//! - **重建路径**：索缺失或落后 → 扫一遍帧结构并逐帧取首条序号，写回索引；
//! - **修复路径**：追加前确保覆盖长度到位，避免新条目与旧条目之间留下未索引的空洞。

use std::path::Path;

use crate::error::EventLogError;
use crate::format::{FrameSpan, HEADER_FRAME_INDEX, Header, scan_file_frames};
use crate::index::{self, FrameIndexEntry};
use crate::read::{decode_header_frame, frame_records, read_snapshot};

/// 用索引还原 header 与下一个序号；索引不可用时退回全量快照并重建索引。
pub(super) fn restore_metadata(
    path: &Path,
    committed: u64,
) -> Result<(Header, u64), EventLogError> {
    if let Some(restored) = index_fast_restore(path, committed) {
        return Ok(restored);
    }
    // 索引缺失、落后或指向的字节解不出来：按日志全量还原一次，并把索引补齐。
    let snapshot = read_snapshot(path)?;
    let header = snapshot.header.clone();
    rebuild(path, committed)?;
    let next = snapshot
        .records
        .last()
        .map_or(0, |record| record.sequence.saturating_add(1));
    Ok((header, next))
}

/// 索引覆盖长度与日志一致时只解压 header 帧与末帧；任何一步不成立都返回 `None`。
fn index_fast_restore(path: &Path, committed: u64) -> Option<(Header, u64)> {
    let index = index::load(path, committed)?;
    let last = index.last()?;
    let header_end = index.header_end(committed);
    let header_bytes = index::read_range(path, 0, header_end).ok()?;
    let header = decode_header_frame(&header_bytes)?;
    let (Ok(start), Ok(end)) = (usize::try_from(last.offset), usize::try_from(last.end())) else {
        return None;
    };
    let mut file = std::fs::File::open(path).ok()?;
    let next = frame_records(&mut file, &FrameSpan { start, end })
        .ok()?
        .last()?
        .sequence
        .saturating_add(1);
    Some((header, next))
}

/// 让索引覆盖到日志的真实末字节：落后就重建，避免新条目与旧条目之间留下空洞。
pub(super) fn ensure(path: &Path, committed: u64) -> Result<(), EventLogError> {
    if committed == 0 {
        return Ok(());
    }
    if index::load(path, committed).is_some() {
        return Ok(());
    }
    rebuild(path, committed)
}

/// 按日志重建索引：只扫描帧结构，再逐帧取首条序号。
///
/// 这是唯一的全量解压路径，只在索引缺失（旧日志、首次升级）或不一致（崩溃后
/// 覆盖长度落后）时发生；重建之后每次读都是索引二分。
fn rebuild(path: &Path, committed: u64) -> Result<(), EventLogError> {
    let mut file = std::fs::File::open(path)?;
    let scan = scan_file_frames(&mut file)?;
    let mut entries = Vec::new();
    for span in scan.frames.iter().skip(HEADER_FRAME_INDEX + 1) {
        let records = frame_records(&mut file, span)?;
        let Some(first) = records.first() else {
            continue;
        };
        let len = u32::try_from(span.end.saturating_sub(span.start))
            .map_err(|_| EventLogError::Corrupt("单帧长度超出索引可表示范围".to_owned()))?;
        entries.push(FrameIndexEntry {
            first_sequence: first.sequence,
            offset: u64::try_from(span.start).unwrap_or(u64::MAX),
            len,
        });
    }
    index::store(path, &entries, committed)
}
