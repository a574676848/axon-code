//! 工作区与车道的真实来源与状态维护。
//!
//! 概念对应：
//! - 工作区来自运行态 Workspace 注册表，名称恒取根目录的文件夹名；
//! - 车道等于 Interaction 的复合归属，同一条车道下的 Interaction 共享
//!   `(workspace, trust scope, channel, destination)`。
//!
//! 表现层不直接读库：数据经 [`crate::lane_source::LaneSource`] 端口取得。
//! 本模块按职责拆成三层，依赖方向单向向内：
//!
//! - [`model`]：展示模型与路径、标识、时间原语，无任何读写；
//! - [`projection`]：只读投影，把运行态整理成界面需要的形状；
//! - [`mutation`]：写入路径，叠加用户自己维护的置顶、归档、重命名与手动顺序。
//!
//! Tauri IPC 命令留在本文件：它们只是把界面参数转交给上面三层，不含逻辑。

mod model;
mod mutation;
mod projection;
#[cfg(test)]
mod tests;

use tauri::State;

use crate::AppState;

pub use model::{LaneDto, WorkspaceDto, strip_extended_prefix};
pub(crate) use model::{generate_uuid, now_millis};
pub use mutation::{
    execute_lane_create, execute_lane_rename, execute_lane_reorder, execute_lane_set_archived,
    execute_lane_set_pinned, execute_workspace_add, execute_workspace_remove,
};
pub use projection::workspace_list_at;

/// Tauri IPC 命令：获取真实工作区与车道列表。
#[tauri::command]
pub fn workspace_list(state: State<'_, AppState>) -> Result<Vec<WorkspaceDto>, String> {
    workspace_list_at(state.lanes.as_ref())
}

/// Tauri IPC 命令：登记一个真实存在的工作区目录。
#[tauri::command]
pub fn workspace_add(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_workspace_add(state.lanes.as_ref(), &path)
}

/// Tauri IPC 命令：移除已登记的工作区。
#[tauri::command]
pub fn workspace_remove(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_workspace_remove(state.lanes.as_ref(), &id)
}

/// Tauri IPC 命令：在指定工作区内新建一条车道。
#[tauri::command]
pub fn lane_create(
    workspace_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_lane_create(state.lanes.as_ref(), &workspace_id)
}

/// Tauri IPC 命令：设置或取消车道置顶。
#[tauri::command]
pub fn lane_set_pinned(
    lane_id: String,
    pinned: bool,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_lane_set_pinned(state.lanes.as_ref(), &lane_id, pinned)
}

/// Tauri IPC 命令：设置或取消车道归档。
#[tauri::command]
pub fn lane_set_archived(
    lane_id: String,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_lane_set_archived(state.lanes.as_ref(), &lane_id, archived)
}

/// Tauri IPC 命令：重命名车道。
#[tauri::command]
pub fn lane_rename(
    lane_id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_lane_rename(state.lanes.as_ref(), &lane_id, &title)
}

/// Tauri IPC 命令：保存某个工作区下车道的自定义顺序。
#[tauri::command]
pub fn lane_reorder(
    workspace_id: String,
    ordered_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_lane_reorder(state.lanes.as_ref(), &workspace_id, ordered_ids)
}
