//! 桌面工作台自身的持久化状态。
//!
//! 只保存桌面外壳拥有的数据：用户登记的工作区、车道的归属与置顶、归档、重命名。
//! 状态文件位于 Axon 运行时根目录（`~/.axon/desktop-state.json`），路径经
//! [`ConfigLoader::default_user_axon_root_path`] 解析，因此 Windows、macOS 与
//! Linux 各自落到用户主目录下，不存在任何写死的绝对路径。
//!
//! 落盘一律复用 `axon_config::atomic_write::replace_file_atomically`，
//! 不新建第二套原子写实现。

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use axon_config::ConfigLoader;
use axon_config::atomic_write::replace_file_atomically;
use serde::{Deserialize, Serialize};

/// 状态文件在运行时根目录下的文件名。
pub const DESKTOP_STATE_FILE: &str = "desktop-state.json";

/// 当前状态结构版本。
pub const DESKTOP_STATE_VERSION: u32 = 1;

/// 用户登记的工作区。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RegisteredWorkspace {
    /// 工作区绝对路径，同时作为稳定标识。
    pub path: String,
    /// 用户可读名称，默认取路径末段。
    pub name: String,
}

/// 桌面工作台持久状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopState {
    /// 结构版本。
    pub version: u32,
    /// 用户登记的工作区清单。
    pub workspaces: Vec<RegisteredWorkspace>,
    /// 车道到所属工作区路径的归属映射。
    pub lane_workspace: HashMap<String, String>,
    /// 已置顶的车道标识。
    pub lane_pinned: Vec<String>,
    /// 已归档的车道标识。
    pub lane_archived: Vec<String>,
    /// 用户重命名后的车道标题。
    pub lane_title: HashMap<String, String>,
    /// 每个工作区下车道的用户自定义顺序（手动排序）。
    pub lane_order: HashMap<String, Vec<String>>,
    /// 本应用新建、尚未产生 Interaction 的车道。
    pub pending_lanes: Vec<PendingLane>,
    /// 上一次在原生目录选择器中选中的目录，用于下次打开时的初始位置。
    pub last_workspace_dir: Option<String>,
    /// 是否发送系统通知；缺省视为开启。
    #[serde(default = "default_notifications_enabled")]
    pub notifications_enabled: bool,
    /// 界面主题标识。
    ///
    /// 空值就是默认的纸白主题：这样历史状态文件不需要迁移，
    /// 任何遗漏写入的路径也会自然落在默认主题上。
    #[serde(default)]
    pub theme: Option<String>,
}

/// 通知默认开启：轮次结束提示是桌面应用的默认预期行为。
const fn default_notifications_enabled() -> bool {
    true
}

/// 本应用新建但还没有真实 Interaction 的车道。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PendingLane {
    /// 车道稳定标识。
    pub lane_id: String,
    /// 所属工作区标识。
    pub workspace_id: String,
    /// 会话表面目标。
    pub destination: String,
    /// 创建时间（毫秒）。
    pub created_at: u64,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            version: DESKTOP_STATE_VERSION,
            workspaces: Vec::new(),
            lane_workspace: HashMap::new(),
            lane_pinned: Vec::new(),
            lane_archived: Vec::new(),
            lane_title: HashMap::new(),
            lane_order: HashMap::new(),
            pending_lanes: Vec::new(),
            last_workspace_dir: None,
            notifications_enabled: default_notifications_enabled(),
            theme: None,
        }
    }
}

/// 解析状态文件路径。
pub fn state_file_path() -> Result<PathBuf, String> {
    let root = ConfigLoader::default_user_axon_root_path()
        .map_err(|error| format!("无法解析 Axon 运行时根目录：{error:?}"))?;
    Ok(root.join(DESKTOP_STATE_FILE))
}

/// 读取状态；文件缺失或损坏时回落到默认状态，不阻断界面。
///
/// 会容忍外部编辑器写入的 UTF-8 BOM：带 BOM 的状态文件若直接解析会失败，
/// 那会让用户已登记的工作区与车道状态被静默丢弃。
#[must_use]
pub fn load_state() -> DesktopState {
    let Ok(path) = state_file_path() else {
        return DesktopState::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return DesktopState::default();
    };
    serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap_or_default()
}

/// 原子写入状态。
pub fn save_state(state: &DesktopState) -> Result<(), String> {
    let path = state_file_path()?;
    let text = serde_json::to_string_pretty(state)
        .map_err(|error| format!("序列化桌面状态失败：{error}"))?;
    replace_file_atomically(&path, text.as_bytes())
        .map_err(|error| format!("原子写入桌面状态失败：{error:?}"))
}

/// 取路径末段作为默认名称，跨平台兼容正反斜杠。
#[must_use]
pub fn directory_label(path: &str) -> String {
    let trimmed = path.trim_end_matches(['/', '\\']);
    let last = trimmed
        .rsplit(['/', '\\'])
        .next()
        .filter(|segment| !segment.is_empty())
        .unwrap_or(trimmed);
    if last.is_empty() {
        path.to_string()
    } else {
        last.to_string()
    }
}
