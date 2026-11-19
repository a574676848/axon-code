//! 工作区与车道的真实来源与状态维护。
//!
//! 概念对应：
//! - 工作区来自运行态 Workspace 注册表，名称恒取根目录的文件夹名；
//! - 车道等于 Interaction 的复合归属，同一条车道下的 Interaction 共享
//!   `(workspace, trust scope, channel, destination)`。
//!
//! 表现层不直接读库：数据经 [`crate::session_source::SessionSource`] 端口取得。
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
use crate::session_source::SessionSource;

pub use model::{SessionDto, WorkspaceDto, is_draft_id, strip_extended_prefix};
pub(crate) use model::{generate_uuid, now_millis};
pub use mutation::{
    SessionCreation, execute_session_create, execute_session_draft, execute_session_rename,
    execute_session_reorder, execute_session_set_archived, execute_session_set_pinned,
    execute_workspace_add, execute_workspace_remove,
};
pub use projection::workspace_list_at;

/// 读取工作区与车道列表，顺带清理失效的待激活记录。
///
/// 清理是维护动作而不是列表前提：它读不到运行态清单时原样返回，列表照常渲染。
pub fn workspace_list_maintained(source: &dyn SessionSource) -> Result<Vec<WorkspaceDto>, String> {
    mutation::prune_pending_sessions(source);
    workspace_list_at(source)
}

/// Tauri IPC 命令：获取真实工作区与车道列表。
#[tauri::command(async)]
pub fn workspace_list(state: State<'_, AppState>) -> Result<Vec<WorkspaceDto>, String> {
    workspace_list_maintained(state.sessions.as_ref())
}

/// Tauri IPC 命令：登记一个真实存在的工作区目录。
#[tauri::command(async)]
pub fn workspace_add(
    path: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_workspace_add(state.sessions.as_ref(), &path)
}

/// Tauri IPC 命令：移除已登记的工作区。
#[tauri::command(async)]
pub fn workspace_remove(
    id: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_workspace_remove(state.sessions.as_ref(), &id)
}

/// Tauri IPC 命令：在指定工作区内登记一条本机草稿会话。
///
/// 草稿只存在于本地状态里，客户端不会因此向服务端建会话；首条消息提交前由
/// [`session_create`] 把它物化成真实会话。
#[tauri::command(async)]
pub fn session_draft(
    workspace_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_session_draft(state.sessions.as_ref(), &workspace_id)
}

/// Tauri IPC 命令：把草稿会话物化成真实会话，返回服务端生成的标识。
#[tauri::command(async)]
pub fn session_create(
    workspace_id: String,
    state: State<'_, AppState>,
) -> Result<SessionCreation, String> {
    execute_session_create(state.sessions.as_ref(), &workspace_id)
}

/// Tauri IPC 命令：设置或取消车道置顶。
#[tauri::command(async)]
pub fn session_set_pinned(
    session_id: String,
    pinned: bool,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_session_set_pinned(state.sessions.as_ref(), &session_id, pinned)
}

/// Tauri IPC 命令：设置或取消车道归档。
#[tauri::command(async)]
pub fn session_set_archived(
    session_id: String,
    archived: bool,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_session_set_archived(state.sessions.as_ref(), &session_id, archived)
}

/// Tauri IPC 命令：重命名车道。
#[tauri::command(async)]
pub fn session_rename(
    session_id: String,
    title: String,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_session_rename(state.sessions.as_ref(), &session_id, &title)
}

/// Tauri IPC 命令：保存某个工作区下车道的自定义顺序。
#[tauri::command(async)]
pub fn session_reorder(
    workspace_id: String,
    ordered_ids: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<WorkspaceDto>, String> {
    execute_session_reorder(state.sessions.as_ref(), &workspace_id, ordered_ids)
}
