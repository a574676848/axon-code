//! 工作区与车道的写入路径：登记、移除与车道维护。
//!
//! 每一次写入都是「先动服务端、再落本地状态、最后重新投影」：本地状态里只保存
//! 用户自己的选择（登记项、置顶、归档、重命名、手动顺序），因此写入永远以
//! 运行态的真实结论为前提，不在本地造第二份事实。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::session_source::{SessionEntry, SessionSource, WorkspaceEntry as RegistryWorkspace};
use crate::state::{
    DesktopState, PendingSession, RegisteredWorkspace, directory_label, load_state, save_state,
};

use super::model::{WorkspaceDto, draft_id, generate_uuid, is_draft_id, normalize, now_millis};
use super::projection::build_workspaces;

/// 待激活会话的有效期：超过这个时长服务端还看不见，就按废弃记录处理。
///
/// 一条草稿只要没提交过消息，服务端就永远不会有它；留着既不会被打开，也不该在
/// 状态文件里越积越多。
const PENDING_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

/// 新建会话的落库结论：工作区快照与服务端刚给出的会话标识。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCreation {
    /// 刷新后的工作区与车道列表。
    pub sessions: Vec<WorkspaceDto>,
    /// 真实的会话标识；界面据此把目标切到这一条。
    pub session_id: String,
}

/// 移除范围：同一条登记项的两个身份。
///
/// 界面持有的是运行态工作区标识（`workspace-…`），而本地登记项以路径为身份。
/// 移除必须从任意一个身份都能定位到同一条登记项，否则「移除工作区」会在
/// 路径比较处静默失配，点了没有任何效果。
pub(super) struct RemovalScope {
    /// 运行态工作区标识。
    ids: Vec<String>,
    /// 归一化后的登记路径。
    paths: Vec<String>,
}

impl RemovalScope {
    /// 由运行态注册表与调用方给出的标识解析移除范围。
    ///
    /// 双向归一：按标识给的、按路径给的，都要落到同一组身份上，
    /// 并且只接受**完全相等**的匹配，路径相近的兄弟工作区绝不能被连带移除。
    pub(super) fn resolve(target: &str, records: &[RegistryWorkspace]) -> Self {
        let mut ids = vec![target.to_string()];
        let mut paths = vec![normalize(Path::new(target))];
        for record in records {
            let root = normalize(Path::new(&record.workspace_root));
            if record.workspace_id != target && !paths.contains(&root) {
                continue;
            }
            if !ids.contains(&record.workspace_id) {
                ids.push(record.workspace_id.clone());
            }
            if !paths.contains(&root) {
                paths.push(root);
            }
        }
        Self { ids, paths }
    }

    /// 从本地状态里摘除范围内的登记项及其派生记录；返回是否真的改动了什么。
    pub(super) fn apply(&self, desktop: &mut DesktopState) -> bool {
        let before = (
            desktop.workspaces.len(),
            desktop.pending_sessions.len(),
            desktop.session_order.len(),
        );
        desktop
            .workspaces
            .retain(|existing| !self.paths.contains(&normalize(Path::new(&existing.path))));
        // 待激活车道与手动顺序都属于被移除的工作区：留下它们会在重新添加同一
        // 目录时冒出一条谁也不认识的空会话。
        desktop
            .pending_sessions
            .retain(|session| !self.ids.contains(&session.workspace_id));
        desktop
            .session_order
            .retain(|workspace_id, _| !self.ids.contains(workspace_id));
        before
            != (
                desktop.workspaces.len(),
                desktop.pending_sessions.len(),
                desktop.session_order.len(),
            )
    }
}

/// 在给定数据源上登记一个工作区目录。
pub fn execute_workspace_add(
    source: &dyn SessionSource,
    path: &str,
) -> Result<Vec<WorkspaceDto>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("工作区路径不能为空".to_string());
    }
    let candidate = PathBuf::from(trimmed);
    if !candidate.is_dir() {
        return Err(format!("目录不存在或不是文件夹：{trimmed}"));
    }
    let canonical = normalize(&candidate);

    // 先完成服务端注册；网关不可用或目录身份解析失败时不写入本地状态，
    // 避免留下后续刷新必然失败的无效登记项。
    source.ensure_workspace(&canonical)?;

    let mut desktop = load_state();
    if !desktop
        .workspaces
        .iter()
        .any(|existing| normalize(Path::new(&existing.path)) == canonical)
    {
        desktop.workspaces.push(RegisteredWorkspace {
            name: directory_label(&canonical),
            path: canonical,
        });
        save_state(&desktop)?;
    }
    build_workspaces(source)
}

/// 在给定数据源上移除已登记的工作区。
pub fn execute_workspace_remove(
    source: &dyn SessionSource,
    id: &str,
) -> Result<Vec<WorkspaceDto>, String> {
    let target = id.trim();
    if target.is_empty() {
        return Err("工作区标识不能为空".to_string());
    }
    // 注册表是把界面标识解析成登记路径的唯一依据；读不到就如实报错，
    // 不做「猜一条登记项来删」的移除。
    let records = source.workspaces()?;
    let scope = RemovalScope::resolve(target, &records);
    let mut desktop = load_state();
    // 一条都没动说明这个标识根本不指向任何登记项：如实失败，
    // 不返回「移除成功」让用户以为点过了。
    if !scope.apply(&mut desktop) {
        return Err(format!("工作区不在已登记清单中：{target}"));
    }
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 新建一条本机草稿会话：只落本地占位，不向服务端登记任何东西。
///
/// 草稿的标识带 [`super::model::DRAFT_ID_PREFIX`] 前缀，服务端从未见过它；首条消息
/// 提交前必须先物化成真实会话（[`execute_session_create`]）。这样点一次「新建会话」
/// 不会在控制面留下一条永远没有内容、又删不掉的空间话。
///
/// **同一工作区同一时刻只需要一条草稿**：点两次不该留下两条空会话，因此已有草稿时
/// 直接复用。
pub fn execute_session_draft(
    source: &dyn SessionSource,
    workspace_id: &str,
) -> Result<Vec<WorkspaceDto>, String> {
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return Err("工作区标识不能为空".to_string());
    }
    let mut desktop = load_state();
    let occupied = desktop
        .pending_sessions
        .iter()
        .any(|session| session.workspace_id == workspace_id && is_draft_id(&session.session_id));
    if !occupied {
        desktop.pending_sessions.push(PendingSession {
            session_id: draft_id(),
            workspace_id: workspace_id.to_string(),
            destination: format!("workbench-{}", generate_uuid()),
            created_at: now_millis(),
        });
        save_state(&desktop)?;
    }
    build_workspaces(source)
}

/// 物化一条会话：向服务端登记并返回真实标识。
///
/// 桌面新建的会话在本机先是草稿，首条消息提交前才来到这里：会话标识由
/// `POST /api/v1/sessions` 生成（客户端不派生），本机把那条草稿记录换成这个标识，
/// 回复目标与创建时间沿用草稿里的那一份，因此物化之后再提交仍然投得回去。
pub fn execute_session_create(
    source: &dyn SessionSource,
    workspace_id: &str,
) -> Result<SessionCreation, String> {
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return Err("工作区标识不能为空".to_string());
    }
    // 只有「服务端还看不见」的待激活记录才值得处理；已经产生 Interaction 的会话
    // 早就是真实会话了，再拿它当新建目标会让「新会话」什么都不做。
    let observed: std::collections::HashSet<String> = source
        .sessions()?
        .into_iter()
        .map(|session| session.session_id)
        .collect();
    // 已登记、只是服务端还没来得及列出的那条连同目标直接复用；草稿留到下面物化，
    // 两种情况都不向服务端多建一条。
    let current = load_state();
    let reusable = current
        .pending_sessions
        .iter()
        .find(|session| {
            session.workspace_id == workspace_id && !observed.contains(&session.session_id)
        })
        .cloned();
    if let Some(record) = reusable
        && !is_draft_id(&record.session_id)
    {
        return Ok(SessionCreation {
            sessions: build_workspaces(source)?,
            session_id: record.session_id,
        });
    }
    // 会话标识由 `POST /api/v1/sessions` 生成，客户端不再本地派生。
    let session_id = source.create_session(workspace_id, None)?;

    /*
     * 写入前重新读取本地状态。
     *
     * 「读 → 网络 → 写」会留下一个丢失更新的窗口：窗口里来自别处的状态变更
     * （另一个窗口、命令行工具、用户直接编辑）都会被这次写入静默覆盖掉
     * （实测过：恢复桌面状态的同时新建会话，会把刚恢复的内容冲掉）。
     * 因此真正要写的那份状态必须在所有网络往返之后取。
     */
    let mut desktop = load_state();
    // 顺手清掉已落地为真实会话的本地待激活记录，避免越积越多。
    desktop
        .pending_sessions
        .retain(|session| !observed.contains(&session.session_id));

    let draft_at = desktop.pending_sessions.iter().position(|session| {
        session.workspace_id == workspace_id && is_draft_id(&session.session_id)
    });
    if let Some(index) = draft_at {
        // 就地换成真实标识：回复目标与创建时间都是用户刚才那一手的延续。
        let draft_id = desktop.pending_sessions[index].session_id.clone();
        desktop.pending_sessions[index].session_id = session_id.clone();
        // 用户在这条草稿上改过的名字与拖拽次序跟着标识一起搬过去：
        // 否则物化之后重命名与排序会静默失效。
        if let Some(title) = desktop.session_title.remove(&draft_id) {
            desktop.session_title.insert(session_id.clone(), title);
        }
        for order in desktop.session_order.values_mut() {
            for entry in order.iter_mut() {
                if *entry == draft_id {
                    *entry = session_id.clone();
                }
            }
        }
    } else {
        desktop
            .pending_sessions
            .retain(|session| session.session_id != session_id);
        desktop.pending_sessions.push(PendingSession {
            session_id: session_id.clone(),
            workspace_id: workspace_id.to_string(),
            destination: format!("workbench-{}", generate_uuid()),
            created_at: now_millis(),
        });
    }
    // 本地只保存用户重命名；未命名车道的占位文案在构建时回落。
    save_state(&desktop)?;

    Ok(SessionCreation {
        sessions: build_workspaces(source)?,
        session_id,
    })
}

/// 清理一条待激活记录是否已经失效。
///
/// 判据分两档：
/// - 服务端已经看得见、且已经有内容：会话表面的渠道、目标与标题都由服务端给出，
///   本地镜像不再承担任何职责；
/// - 服务端一直看不见、且超过有效期：那是一条废弃的草稿（或更早版本留下的孤儿
///   记录），留着既不会被打开，也只会在状态文件里越积越多。
fn pending_is_stale(pending: &PendingSession, sessions: &[SessionEntry], now_ms: u64) -> bool {
    match sessions
        .iter()
        .find(|session| session.session_id == pending.session_id)
    {
        Some(session) => session.interaction_count > 0,
        None => now_ms.saturating_sub(pending.created_at) >= PENDING_TTL_MS,
    }
}

/// 按运行态清单清理待激活记录；返回是否真的改动了状态。
pub(super) fn prune_pending(
    desktop: &mut DesktopState,
    sessions: &[SessionEntry],
    now_ms: u64,
) -> bool {
    let before = desktop.pending_sessions.len();
    desktop
        .pending_sessions
        .retain(|pending| !pending_is_stale(pending, sessions, now_ms));
    desktop.pending_sessions.len() != before
}

/// 读取运行态清单并落盘清理一次。
///
/// 清单读不到时原样返回、一条都不删：网关不可达不代表这些会话不存在，凭一次读失败
/// 丢掉本地记录等于让用户下次打开时凭空少一条会话。
pub(super) fn prune_pending_sessions(source: &dyn SessionSource) {
    let Ok(sessions) = source.sessions() else {
        return;
    };
    let mut desktop = load_state();
    if prune_pending(&mut desktop, &sessions, now_millis()) {
        let _ = save_state(&desktop);
    }
}

/// 在给定数据源上设置或取消车道置顶。
///
/// 先写运行态、再同步本地镜像：置顶是会话在 `sessions` 表里的状态，三端读的是同一份，
/// 因此失败必须原样上报（网关不可达、会话尚未登记），不能只在本地改一个标记让用户
/// 以为已经生效。本地镜像继续维护，是为了让「还没有产生回合、因而不在会话清单里」
/// 的新建会话保持住用户刚点下的选择。
pub fn execute_session_set_pinned(
    source: &dyn SessionSource,
    session_id: &str,
    pinned: bool,
) -> Result<Vec<WorkspaceDto>, String> {
    crate::session_menu::patch(session_id, Some(pinned), None)?;
    let mut desktop = load_state();
    desktop.session_pinned.retain(|id| id != session_id);
    if pinned {
        desktop.session_pinned.push(session_id.to_string());
    }
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 在给定数据源上设置或取消车道归档。
///
/// 归档同样先写运行态：归档后从默认列表消失的依据是 `sessions.archived`，
/// 写不进去就必须让用户看见，否则「归档成功」只是本地假装。
pub fn execute_session_set_archived(
    source: &dyn SessionSource,
    session_id: &str,
    archived: bool,
) -> Result<Vec<WorkspaceDto>, String> {
    crate::session_menu::patch(session_id, None, Some(archived))?;
    let mut desktop = load_state();
    desktop.session_archived.retain(|id| id != session_id);
    if archived {
        desktop.session_archived.push(session_id.to_string());
    }
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 在给定数据源上重命名车道。
pub fn execute_session_rename(
    source: &dyn SessionSource,
    session_id: &str,
    title: &str,
) -> Result<Vec<WorkspaceDto>, String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err("车道标题不能为空".to_string());
    }
    let mut desktop = load_state();
    desktop
        .session_title
        .insert(session_id.to_string(), trimmed.to_string());
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 在给定数据源上保存某个工作区下车道的自定义顺序。
pub fn execute_session_reorder(
    source: &dyn SessionSource,
    workspace_id: &str,
    ordered_ids: Vec<String>,
) -> Result<Vec<WorkspaceDto>, String> {
    if ordered_ids.is_empty() {
        return Err("排序结果不能为空".to_string());
    }
    let mut desktop = load_state();
    desktop
        .session_order
        .insert(workspace_id.to_string(), ordered_ids);
    save_state(&desktop)?;
    build_workspaces(source)
}
