//! 工作区与车道的展示模型，以及路径、标识与时间原语。
//!
//! 这一层只有数据形状与纯函数：路径归一化、目录名提取与标识生成。
//! 它不读状态、不访问网关，因此投影与写入两侧可以共用同一套口径。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 车道数据模型（等同通用工作台的会话）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaneDto {
    /// 车道稳定标识。
    pub id: String,
    /// 车道显示标题。
    pub title: String,
    /// 会话表面渠道，例如 tui、feishu 或本应用的 desktop。
    pub channel: String,
    /// 会话表面目标。
    pub destination: String,
    /// 受众边界标签。
    pub audience: String,
    /// 该车道下的 Interaction 总数。
    pub interaction_count: u64,
    /// 该车道当前待答复的提问数量；无待答或服务端未提供时为 0。
    pub pending_question_count: u64,
    /// 车道创建时间戳（毫秒）。
    pub created_at: u64,
    /// 最后更新时间戳（毫秒）。
    pub updated_at: u64,
    /// 是否置顶。
    pub pinned: bool,
    /// 是否归档。
    pub archived: bool,
    /// 是否尚未产生任何 Interaction 的新建车道。
    pub pending: bool,
    /// 该车道当前是否有 Interaction 正在处理中。
    pub running: bool,
}

/// 工作区数据模型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceDto {
    /// 工作区稳定标识。
    pub id: String,
    /// 工作区显示名称，恒为根目录的文件夹名。
    pub name: String,
    /// 物理路径。
    pub path: String,
    /// 是否为当前运行工作区。
    pub active: bool,
    /// 是否可移除。
    pub removable: bool,
    /// 所属车道列表。
    pub lanes: Vec<LaneDto>,
}

/// 当前运行工作区目录（跨平台，取自进程实际工作目录）。
pub(super) fn runtime_workspace_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// 生成一个 UUID v4 形状的唯一标识，避免为零散标识引入外部依赖。
pub(crate) fn generate_uuid() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0_u128, |duration| duration.as_nanos());
    let pid = u128::from(std::process::id());
    let stack_entropy = (&nanos as *const u128 as usize) as u128;
    let mixed = nanos.rotate_left(17).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ pid.wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ stack_entropy.rotate_right(11);
    let hex = format!("{mixed:032x}");
    format!(
        "{}-{}-4{}-a{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[13..16],
        &hex[17..20],
        &hex[20..32]
    )
}

/// 去掉 Windows 扩展长度前缀，并把分隔符统一为当前平台的形式。
///
/// 注册表里历史上存在 `\\?\e:\...` 与 `//?/e:/...` 两种写法，
/// 两者都必须还原成可直接展示与回传的普通路径。
pub fn strip_extended_prefix(text: &str) -> String {
    let trimmed = if let Some(rest) = text.strip_prefix(r"\\?\") {
        match rest.strip_prefix(r"UNC\") {
            Some(share) => format!(r"\\{share}"),
            None => rest.to_string(),
        }
    } else if let Some(rest) = text.strip_prefix("//?/") {
        rest.to_string()
    } else {
        text.to_string()
    };

    if cfg!(windows) {
        trimmed.replace('/', "\\")
    } else {
        trimmed
    }
}

/// 归一化路径用于比较：能规范化则规范化，否则保持原样。
///
/// 注册表给出的根目录带扩展长度前缀、本地登记项是普通路径，两者只有归一化之后
/// 才可比；同一目录的两种写法必须落到同一个字符串上，否则身份比较会静默失配。
pub(super) fn normalize(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    strip_extended_prefix(&resolved.to_string_lossy())
}

/// 当前时间（毫秒）。
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}
