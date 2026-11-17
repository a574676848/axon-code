//! 事件日志的旁挂帧索引：模型与查询。
//!
//! 日志物理上是若干互相独立的 zstd 帧串接，而 zstd 帧头不携带事件序号：
//! 「第 N 条事件落在哪一帧」只能靠解压帧本身来回答。逐次解压会把分页读取退化成
//! 整条日志冷读：实测 0.6 MB 日志的交互历史首页要 650 ms，日志再长就是秒级。
//!
//! 索引把「首条序号 → 帧字节区间」写成固定宽度记录，与日志同目录旁挂：
//!
//! - **写句柄**在追加每一帧后追加一条记录，最后更新文件头里的「已覆盖日志长度」。
//!   顺序固定为「帧 → 条目 → 覆盖长度」，因此崩溃只会让索引落后，不会指向不存在的字节。
//! - **读路径**二分索引直接定位起点帧，再顺序解压到够量；索引缺失或落后时退化为
//!   从首帧顺序解压并跳过起点之前的部分，仍然是单趟，不做重复扫描。
//!
//! 每条记录还带一个**交互标签**（`interaction_id` 的 32 位指纹，0 表示跨交互或无
//! 归属）：一次追加批次就是一个交互的一帧，因此「这条交互的帧」在索引里是连续一段，
//! 按交互取页不必把整段会话的帧都解压一遍。标签只是缩小解压范围的粗筛，命中与否
//! 最终仍由解出的记录逐条判定，指纹碰撞只多解压一帧、不会返回错页。
//!
//! 索引是纯派生数据：损坏、缺失或落后一律按日志重建，不参与任何事实判定。
//! 落盘格式与读写见 [`crate::index_file`]。

use std::path::{Path, PathBuf};

/// 落盘格式与读写见同目录的 `index_file.rs`。
#[path = "index_file.rs"]
mod index_file;
pub(crate) use index_file::{append, load, read_range, store};

/// 索引文件名后缀。
pub(crate) const SUFFIX: &str = ".idx";

/// 一帧在日志里的定位信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameIndexEntry {
    /// 该帧首条事件的序号。
    pub(crate) first_sequence: u64,
    /// 该帧在日志文件中的起始字节。
    pub(crate) offset: u64,
    /// 该帧的字节长度。
    pub(crate) len: u32,
    /// 该帧归属的交互指纹；0 表示跨交互（或整帧都没有归属）。
    pub(crate) interaction_tag: u32,
}

impl FrameIndexEntry {
    /// 该帧结束处的字节偏移。
    pub(crate) fn end(&self) -> u64 {
        self.offset.saturating_add(u64::from(self.len))
    }

    /// 该帧是否可能承载目标交互：跨交互帧（0）必须保守地算作可能。
    pub(crate) fn may_hold(&self, tag: u32) -> bool {
        self.interaction_tag == 0 || self.interaction_tag == tag
    }
}

/// 交互标识的 32 位指纹；空标识与哈希到 0 的情况都折回 0（跨交互）。
///
/// FNV-1a：实现短、对短字符串分布均匀。指纹只用于缩小解压范围，正确性由逐条判定
/// 兜底，因此不需要密码学强度。
pub(crate) fn interaction_tag(interaction_id: Option<&str>) -> u32 {
    let Some(interaction_id) = interaction_id.filter(|value| !value.is_empty()) else {
        return 0;
    };
    let mut hash = 0x811c_9dc5_u32;
    for byte in interaction_id.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// 一份已校验的帧索引（只覆盖数据帧，不含 header 帧）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FrameIndex {
    entries: Vec<FrameIndexEntry>,
}

impl FrameIndex {
    /// 用已校验的条目构造索引。
    pub(crate) fn new(entries: Vec<FrameIndexEntry>) -> Self {
        Self { entries }
    }

    /// 数据帧条目，按日志顺序排列。
    pub(crate) fn entries(&self) -> &[FrameIndexEntry] {
        &self.entries
    }

    /// 末条数据帧。
    pub(crate) fn last(&self) -> Option<FrameIndexEntry> {
        self.entries.last().copied()
    }

    /// header 帧的结束偏移；没有数据帧时等于整个日志长度。
    pub(crate) fn header_end(&self, log_len: u64) -> u64 {
        self.entries.first().map_or(log_len, |entry| entry.offset)
    }

    /// 定位「首条序号不大于 `after` 的最后一帧」；`after` 早于首帧时返回首帧。
    ///
    /// 条目按首条序号单调递增，因此这里是标准二分：读路径据此直接 seek 到起点帧，
    /// 不必先解压前面的任何一帧。
    pub(crate) fn locate(&self, after: u64) -> Option<FrameIndexEntry> {
        let first = self.entries.first().copied()?;
        if after < first.first_sequence {
            return Some(first);
        }
        let mut low = 0usize;
        let mut high = self.entries.len();
        let mut found = first;
        while low < high {
            let middle = low + (high - low) / 2;
            let entry = self.entries.get(middle).copied()?;
            if entry.first_sequence <= after {
                found = entry;
                low = middle.saturating_add(1);
            } else {
                high = middle;
            }
        }
        Some(found)
    }
}

/// 索引文件路径：与日志同路径追加 `.idx`。
pub(crate) fn index_path(log_path: &Path) -> PathBuf {
    let mut name = log_path.as_os_str().to_os_string();
    name.push(SUFFIX);
    PathBuf::from(name)
}
