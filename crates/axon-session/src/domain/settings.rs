//! Thread 设置快照。

use serde::{Deserialize, Serialize};

/// Thread 运行设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadSettings {
    /// 工作目录。
    pub cwd: String,
    /// Provider 名称。
    pub provider: String,
    /// 模型名称。
    pub model: String,
    /// 选中的 skill。
    pub selected_skill_name: Option<String>,
}
