//! Daemon 运行目录路径。

use std::path::{Path, PathBuf};

/// 运行时目录集合。
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    /// Axon 根目录。
    pub root: PathBuf,
    /// 持久运行数据目录。
    pub data: PathBuf,
    /// Daemon 日志目录。
    pub logs: PathBuf,
}

impl RuntimePaths {
    /// 从 Axon 根目录创建路径集合。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let data = root.join("data");
        Self {
            logs: data.join("logs"),
            data,
            root,
        }
    }
    /// 返回 runtime.sqlite3 路径。
    #[must_use]
    pub fn runtime_database(&self) -> PathBuf {
        self.data.join("runtime.sqlite3")
    }
    /// 以 Path 形式借用根目录。
    #[must_use]
    pub fn root_path(&self) -> &Path {
        &self.root
    }
}
