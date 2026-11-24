//! 文件变更协议模型。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Codex 风格的最小文件变更模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileChange {
    /// 新增文件，持有完整新增内容。
    Add {
        /// 新文件内容。
        content: String,
    },
    /// 删除文件，持有删除前内容。
    Delete {
        /// 删除前内容。
        content: String,
    },
    /// 修改文件，重命名也折叠为带目标路径的 update。
    Update {
        /// unified diff 文本。
        unified_diff: String,
        /// 重命名后的目标路径；普通修改为 `None`。
        move_path: Option<PathBuf>,
    },
}
