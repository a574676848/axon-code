//! 桌面工作台自身的持久化状态：车道归属、置顶、归档、重命名与客户端偏好。
//!
//! 从 v2 起状态分三层（合同 §4.4）：
//!
//! | 层 | 字段 | 归属 |
//! |---|---|---|
//! | CLIENT | 主题、通知、最近目录、生效档案 id、引导版本 | 落盘，全局一份 |
//! | PROFILE 投影 | 工作区与车道那一组字段 | 落盘，每档一份 |
//! | 运行态 | 能力位、探活迟滞、订阅表 | 不落盘 |
//!
//! 本模块对外仍然是**一份视图**：调用方拿到的 [`DesktopState`] 已经把客户端层与
//! 当前档案投影合成好了，因此既有调用点不需要知道分层的存在。真正落盘与迁移在
//! [`crate::connection::store`]，两份实现只有一份。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::connection::store::{self, ClientLayer, ProfileProjection};

/// v2 状态文件的文件名。
pub const DESKTOP_STATE_FILE: &str = store::V2_FILE_NAME;

/// 当前状态结构版本。
pub const DESKTOP_STATE_VERSION: u32 = 2;

/// 用户登记的工作区。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RegisteredWorkspace {
    /// 工作区绝对路径，同时作为稳定标识。
    pub path: String,
    /// 用户可读名称，默认取路径末段。
    pub name: String,
}

/// 桌面工作台的持久状态视图。
#[derive(Debug, Clone)]
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
    pub notifications_enabled: bool,
    /// 界面主题标识；空值就是默认的纸白主题。
    pub theme: Option<String>,
}

/// 通知默认开启：轮次结束提示是桌面应用的默认预期行为。
const fn default_notifications_enabled() -> bool {
    true
}

/// 本应用新建但还没有真实 Interaction 的车道。
///
/// 落盘键名取 camelCase，与 v2 状态文件其余字段同一口径。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
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

impl DesktopState {
    /// 由客户端层与一份档案投影合成视图。
    #[must_use]
    pub fn from_parts(client: &ClientLayer, projection: &ProfileProjection) -> Self {
        Self {
            version: DESKTOP_STATE_VERSION,
            workspaces: projection.workspaces.clone(),
            lane_workspace: projection.lane_workspace.clone(),
            lane_pinned: projection.lane_pinned.clone(),
            lane_archived: projection.lane_archived.clone(),
            lane_title: projection.lane_title.clone(),
            lane_order: projection.lane_order.clone(),
            pending_lanes: projection.pending_lanes.clone(),
            last_workspace_dir: client.last_workspace_dir.clone(),
            notifications_enabled: client.notifications_enabled,
            theme: client.theme.clone(),
        }
    }

    /// 取出可直接写回档案的投影。
    #[must_use]
    pub fn to_projection(&self) -> ProfileProjection {
        ProfileProjection {
            workspaces: self.workspaces.clone(),
            lane_workspace: self.lane_workspace.clone(),
            lane_pinned: self.lane_pinned.clone(),
            lane_archived: self.lane_archived.clone(),
            lane_title: self.lane_title.clone(),
            lane_order: self.lane_order.clone(),
            pending_lanes: self.pending_lanes.clone(),
        }
    }
}

/// 解析状态文件路径。
///
/// # Errors
///
/// 当前实现不会失败：客户端根由平台环境或 `AXON_CLIENT_ROOT` 决定，缺失时按空路径
/// 拼出一个相对文件名，调用方随后读到的仍是「文件不存在」。保留 `Result` 形态是
/// 为了不改变既有调用点。
pub fn state_file_path() -> Result<std::path::PathBuf, String> {
    Ok(store::state_file_path())
}

/// 读取状态视图。
///
/// 全过程只读进程内已加载的 v2 文档：状态文件在启动时读一次，之后每次取值都不再
/// 触碰磁盘。文件缺失、结构为未来版本或已损坏的情形都由 [`crate::connection::store`]
/// 在加载阶段处置完毕，这里只做合成。
#[must_use]
pub fn load_state() -> DesktopState {
    let runtime = crate::connection::runtime();
    let document = runtime.document();
    let profile_id = runtime
        .active_profile_id()
        .unwrap_or_else(|| store::LOCAL_PROFILE_ID.to_owned());
    DesktopState::from_parts(&document.client, &document.projection(&profile_id))
}

/// 写入状态视图：客户端层与当前档案投影各自落到自己的位置。
///
/// # Errors
///
/// 只读模式（状态文件来自更高结构版本）或写盘失败。
pub fn save_state(state: &DesktopState) -> Result<(), String> {
    let runtime = crate::connection::runtime();
    let profile_id = runtime
        .active_profile_id()
        .unwrap_or_else(|| store::LOCAL_PROFILE_ID.to_owned());
    let projection = state.to_projection();
    runtime.update_document(|document| {
        document.client.theme = state.theme.clone();
        document.client.notifications_enabled = state.notifications_enabled;
        document.client.last_workspace_dir = state.last_workspace_dir.clone();
        document.set_projection(&profile_id, projection.clone());
        Ok(())
    })
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

#[cfg(test)]
mod tests {
    use super::{DesktopState, directory_label};

    #[test]
    fn state_round_trips_through_the_projection() {
        let mut state = DesktopState {
            theme: Some("obsidian".to_owned()),
            notifications_enabled: false,
            ..DesktopState::default()
        };
        state.lane_pinned.push("lane-1".to_owned());
        let projection = state.to_projection();
        assert_eq!(projection.lane_pinned, vec!["lane-1".to_owned()]);

        let client = crate::connection::store::ClientLayer {
            theme: state.theme.clone(),
            notifications_enabled: state.notifications_enabled,
            last_workspace_dir: state.last_workspace_dir.clone(),
            ..crate::connection::store::ClientLayer::default()
        };
        let rebuilt = DesktopState::from_parts(&client, &projection);
        assert_eq!(rebuilt.version, super::DESKTOP_STATE_VERSION);
        assert_eq!(rebuilt.theme.as_deref(), Some("obsidian"));
        assert!(!rebuilt.notifications_enabled);
        assert_eq!(rebuilt.lane_pinned, vec!["lane-1".to_owned()]);
    }

    #[test]
    fn pending_lane_serializes_with_camel_case_keys() {
        let lane = super::PendingLane {
            lane_id: "lane-1".to_owned(),
            workspace_id: "ws".to_owned(),
            destination: "s1".to_owned(),
            created_at: 5,
        };
        let value = serde_json::to_value(&lane).expect("应可序列化");
        assert_eq!(value["laneId"], "lane-1");
        assert_eq!(value["workspaceId"], "ws");
        assert_eq!(value["createdAt"], 5);
    }

    #[test]
    fn directory_label_extracts_the_last_segment() {
        assert_eq!(directory_label("D:/work/alpha/"), "alpha");
        assert_eq!(directory_label("D:\\work\\beta"), "beta");
        assert_eq!(directory_label(""), "");
    }
}
