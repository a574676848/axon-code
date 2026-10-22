//! Feishu 远控运行态目录。

use std::path::{Path, PathBuf};

const REMOTE_RUNTIME_DIR: &str = "remote";
const FEISHU_RUNTIME_DIR: &str = "feishu";
const RUNTIME_DATA_DIR: &str = "data";

/// 解析 Feishu 远控专用运行态根目录。
///
/// 所有 Feishu gateway、治理、processing、媒体缓存与回执文件都应落在
/// `~/.axon/data/remote/feishu/` 下，避免污染 Axon 用户级根目录。
#[must_use]
pub fn remote_feishu_runtime_root(root: impl AsRef<Path>) -> PathBuf {
    root.as_ref()
        .join(RUNTIME_DATA_DIR)
        .join(REMOTE_RUNTIME_DIR)
        .join(FEISHU_RUNTIME_DIR)
}
