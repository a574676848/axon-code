//! 帧索引的落盘格式与读写。
//!
//! 文件布局：24 字节文件头 + 若干 24 字节定宽记录。
//!
//! ```text
//! 0  magic "AXONEVLI"      8
//! 8  version u32           4
//! 12 reserved u32          4
//! 16 covered_log_len u64   8      索引覆盖到的日志字节数
//! 24 entry[0]: first_sequence u64 | offset u64 | len u32 | interaction_tag u32
//! ```
//!
//! 「已覆盖日志长度」是唯一的新鲜度判据：它必须与日志真实长度逐字节相等，
//! 否则整份索引按落后处理（读路径退化为单趟顺序解压，写路径重建）。
//!
//! 版本 2 起每条记录带交互指纹；版本 1 的索引一律按不兼容处理（读侧退化、写侧重建），
//! 格式变更不需要迁移代码。

use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
use std::path::Path;

use crate::error::EventLogError;
use crate::index::{FrameIndex, FrameIndexEntry, index_path};

/// 索引文件魔数；识别不出就不碰。
const MAGIC: &[u8; 8] = b"AXONEVLI";
/// 索引格式版本；布局变更必须递增。版本 2 起记录带交互指纹。
const VERSION: u32 = 2;
/// 文件头字节数。
const HEADER_LEN: usize = 24;
/// 「已覆盖日志长度」字段在文件头里的偏移。
const COVERED_OFFSET: u64 = 16;
/// 单条记录的字节数：首条序号 8 + 帧偏移 8 + 帧长度 4 + 交互指纹 4。
const ENTRY_LEN: usize = 24;

impl FrameIndexEntry {
    fn encode(&self) -> [u8; ENTRY_LEN] {
        let mut bytes = [0u8; ENTRY_LEN];
        bytes[0..8].copy_from_slice(&self.first_sequence.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.offset.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.len.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.interaction_tag.to_le_bytes());
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        let first_sequence = u64::from_le_bytes(bytes.get(0..8)?.try_into().ok()?);
        let offset = u64::from_le_bytes(bytes.get(8..16)?.try_into().ok()?);
        let len = u32::from_le_bytes(bytes.get(16..20)?.try_into().ok()?);
        let interaction_tag = u32::from_le_bytes(bytes.get(20..24)?.try_into().ok()?);
        Some(Self {
            first_sequence,
            offset,
            len,
            interaction_tag,
        })
    }
}

/// 读取并校验索引；只有「覆盖长度与日志完全一致且结构自洽」时才被采纳。
pub(crate) fn load(log_path: &Path, log_len: u64) -> Option<FrameIndex> {
    let bytes = std::fs::read(index_path(log_path)).ok()?;
    if bytes.len() < HEADER_LEN || bytes.get(0..8)? != MAGIC {
        return None;
    }
    let version = u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?);
    if version != VERSION {
        return None;
    }
    let covered = u64::from_le_bytes(bytes.get(16..24)?.try_into().ok()?);
    if covered != log_len {
        return None;
    }
    let mut entries: Vec<FrameIndexEntry> = Vec::new();
    let mut previous: Option<FrameIndexEntry> = None;
    // 尾条目可能只写了一半：只接受完整的定宽记录。
    for chunk in bytes[HEADER_LEN..].as_chunks::<ENTRY_LEN>().0 {
        let entry = FrameIndexEntry::decode(chunk)?;
        if entry.len == 0 {
            return None;
        }
        if let Some(previous) = previous
            && (entry.first_sequence <= previous.first_sequence || entry.offset != previous.end())
        {
            // 序号必须严格递增、偏移必须首尾相接：有空洞或重叠的索引撑不起二分。
            return None;
        }
        if entry.end() > log_len {
            return None;
        }
        entries.push(entry);
        previous = Some(entry);
    }
    /*
     * 覆盖长度齐、条目却少一条的索引会「看起来新鲜」：掉电时覆盖长度所在的第一页可能
     * 先落盘，而新条目所在页未落盘。这样的索引会让尾号退到上一帧，恢复路径据此写出
     * 重号。因此末条必须正好封住覆盖长度，否则整份索引不可信，按日志重建。
     */
    if let Some(last) = entries.last()
        && last.end() != covered
    {
        return None;
    }
    Some(FrameIndex::new(entries))
}

/// 整体重写索引：先写临时文件再原子重命名，避免半份索引留在日志旁。
pub(crate) fn store(
    log_path: &Path,
    entries: &[FrameIndexEntry],
    log_len: u64,
) -> Result<(), EventLogError> {
    let path = index_path(log_path);
    let mut bytes = Vec::with_capacity(HEADER_LEN + entries.len().saturating_mul(ENTRY_LEN));
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&log_len.to_le_bytes());
    for entry in entries {
        bytes.extend_from_slice(&entry.encode());
    }
    let mut temporary = path.clone().into_os_string();
    temporary.push(".tmp");
    let temporary = std::path::PathBuf::from(temporary);
    {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_data()?;
    }
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

/// 追加一条索引记录并刷新覆盖长度；索引文件为空时先写文件头。
///
/// 顺序固定为「条目 → 覆盖长度」：崩溃只会让覆盖长度落后，校验失败后按日志重建。
pub(crate) fn append(
    handle: &mut std::fs::File,
    index_len: &mut u64,
    entry: FrameIndexEntry,
    covered_log_len: u64,
) -> Result<(), EventLogError> {
    if *index_len == 0 {
        handle.write_all(MAGIC)?;
        handle.write_all(&VERSION.to_le_bytes())?;
        handle.write_all(&0u32.to_le_bytes())?;
        handle.write_all(&covered_log_len.to_le_bytes())?;
        *index_len = HEADER_LEN as u64;
    }
    handle.seek(SeekFrom::End(0))?;
    handle.write_all(&entry.encode())?;
    handle.seek(SeekFrom::Start(COVERED_OFFSET))?;
    handle.write_all(&covered_log_len.to_le_bytes())?;
    handle.seek(SeekFrom::End(0))?;
    handle.flush()?;
    *index_len = index_len.saturating_add(ENTRY_LEN as u64);
    Ok(())
}

/// 只读一段字节区间。
pub(crate) fn read_range(path: &Path, start: u64, len: u64) -> Result<Vec<u8>, EventLogError> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    file.take(len).read_to_end(&mut buffer)?;
    Ok(buffer)
}
