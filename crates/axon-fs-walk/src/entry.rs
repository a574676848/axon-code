//! 遍历条目的稳定投影。

use std::path::PathBuf;

/// 文件系统条目类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// 普通文件。
    File,
    /// 目录。
    Dir,
    /// 符号链接。
    Symlink,
    /// 其他平台相关条目。
    Other,
}

/// fd 风格遍历输出的单个条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkEntry {
    /// 绝对或调用方传入根目录下的完整路径。
    pub path: PathBuf,
    /// 相对遍历根目录的路径。
    pub relative: PathBuf,
    /// 条目类型。
    pub kind: EntryKind,
    /// 相对根目录的深度，根目录为 0。
    pub depth: usize,
    /// 文件大小；非文件条目为 `None`。
    pub file_size: Option<u64>,
}

impl WalkEntry {
    pub(crate) fn new(
        path: PathBuf,
        relative: PathBuf,
        kind: EntryKind,
        depth: usize,
        file_size: Option<u64>,
    ) -> Self {
        Self {
            path,
            relative,
            kind,
            depth,
            file_size,
        }
    }
}
