//! 工作区与车道的只读投影：把真实运行态整理成界面需要的形状。
//!
//! 这一层只读：它读本地登记状态、读运行态注册表与车道清单，输出展示模型。
//! 任何写入都在 [`super::mutation`] 里完成，因此这里不存在第二份可写副本。

use std::collections::HashMap;
use std::path::Path;

use crate::session_menu::SessionMenuEntry;
use crate::session_source::{DESKTOP_SESSION_CHANNEL, SessionSource};
use crate::state::{directory_label, load_state};

use super::model::{SessionDto, WorkspaceDto, is_draft_id, normalize, runtime_workspace_dir};

/// 工作区在界面上的登记项。
struct WorkspaceEntry {
    /// 工作区稳定标识。
    id: String,
    /// 显示名称。
    name: String,
    /// 物理路径。
    path: String,
    /// 是否为当前运行工作区。
    active: bool,
    /// 是否可移除。
    removable: bool,
}

/// 按置顶块与手动顺序排列一个工作区下的车道。
///
/// 置顶块永远在最前，块内按用户置顶的先后；其余车道若存在手动顺序，
/// 则严格按该顺序，未登记的新车道按最后更新时间追加到末尾。
fn order_sessions(sessions: &mut [SessionDto], pinned: &[String], manual: Option<&Vec<String>>) {
    let rank = |id: &str| -> usize {
        pinned
            .iter()
            .position(|entry| entry == id)
            .unwrap_or(usize::MAX)
    };
    sessions.sort_by(|left, right| {
        right
            .pinned
            .cmp(&left.pinned)
            .then_with(|| rank(&left.id).cmp(&rank(&right.id)))
            .then_with(|| match manual {
                Some(order) => {
                    let left_at = order.iter().position(|id| id == &left.id);
                    let right_at = order.iter().position(|id| id == &right.id);
                    match (left_at, right_at) {
                        (Some(a), Some(b)) => a.cmp(&b),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => right.updated_at.cmp(&left.updated_at),
                    }
                }
                None => right.updated_at.cmp(&left.updated_at),
            })
    });
}

/// 组装工作区登记项。
///
/// 只列出用户在应用内显式添加的工作区：运行态注册表里的历史记录、
/// 当前进程工作目录与 Axon 家目录都不自动出现在侧栏。
fn workspace_entries(source: &dyn SessionSource) -> Result<Vec<WorkspaceEntry>, String> {
    let state = load_state();
    let active_path = normalize(&runtime_workspace_dir());

    // 运行态注册表既用于把用户添加的目录解析成其 workspace_id，
    // 也用于发现尚未注册的目录：未注册目录必须先完成服务端注册，
    // 否则它派生出的车道会带上路径字符串当工作区标识，既无法与其它渠道对齐，
    // 也会在提交时被注册校验拒绝。
    let mut records = source.workspaces()?;
    let mut resolve_id = |path: &str| -> Result<String, String> {
        if let Some(record) = records
            .iter()
            .find(|record| normalize(Path::new(&record.workspace_root)) == path)
        {
            return Ok(record.workspace_id.clone());
        }
        let registered = source.ensure_workspace(path)?;
        let workspace_id = registered.workspace_id.clone();
        records.push(registered);
        Ok(workspace_id)
    };

    let mut entries: Vec<WorkspaceEntry> = Vec::new();
    for registered in &state.workspaces {
        if registered.path.is_empty() {
            continue;
        }
        let path = normalize(Path::new(&registered.path));
        if entries.iter().any(|entry| entry.path == path) {
            continue;
        }
        entries.push(WorkspaceEntry {
            id: resolve_id(&path)?,
            // 名称恒取根目录的文件夹名。
            name: directory_label(&path),
            path: path.clone(),
            active: path == active_path,
            removable: true,
        });
    }

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.active));
    Ok(entries)
}

/// 车道是否已归档。
///
/// 运行态清单给的这一位是权威值：归档动作可能来自命令行、TUI 或另一个窗口，它们的
/// 事实只落在会话表里。本地镜像只是旧版本后台不下发这一位时的兜底，不能让「桌面自己
/// 存过一份状态」盖住三端共用的那一份。
pub(super) fn archived_of(entry: &crate::session_source::SessionEntry, mirror: &[String]) -> bool {
    entry.archived || mirror.iter().any(|id| id == &entry.session_id)
}

/// 按运行态会话清单校正车道的状态、标题与活动时刻。
///
/// 三件事都以 `sessions` 表为准：置顶与归档取表里的值，标题取表里的会话标题
/// （用户在本机的重命名属于本地偏好，仍然优先），最近活动时间取表里的时间戳。
/// 活动时间对齐之后，[`order_sessions`] 按它排序得到的次序就是网关那条
/// 「置顶优先、再按最近活动倒序」的结果，客户端不再另算一套顺序。
///
/// 不在清单里的车道（本端新建、尚未产生首条消息）保持本地镜像不动：会话还不存在时
/// 没有可对齐的事实，此时硬塞一个默认值只会把用户刚点下的置顶抹掉。
pub(super) fn apply_session_facts(
    sessions: &mut [SessionDto],
    facts: &[SessionMenuEntry],
    renamed: &HashMap<String, String>,
) {
    if facts.is_empty() {
        return;
    }
    let index: HashMap<&str, &SessionMenuEntry> = facts
        .iter()
        .map(|fact| (fact.session_id.as_str(), fact))
        .collect();
    for session in sessions.iter_mut() {
        let Some(fact) = index.get(session.id.as_str()) else {
            continue;
        };
        session.pinned = fact.pinned;
        session.archived = fact.archived;
        // 零表示没有任何活动观测，此时保留会话自己报的时间而不是把时间清成 1970。
        if fact.last_activity_at_ms > 0 {
            session.updated_at = fact.last_activity_at_ms;
        }
        let custom_name = renamed
            .get(&session.id)
            .is_some_and(|title| !title.trim().is_empty());
        if !custom_name && !fact.title.trim().is_empty() {
            session.title = fact.title.clone();
        }
    }
}

/// 组装全部工作区及其车道。
pub(super) fn build_workspaces(source: &dyn SessionSource) -> Result<Vec<WorkspaceDto>, String> {
    let state = load_state();
    let entries = workspace_entries(source)?;
    /*
     * 会话清单只读一次：它同时决定置顶、归档、标题与次序。按工作区分别再取会把同一份
     * 事实读多遍，两次之间还可能被另一端改写。
     *
     * 读不到时退回本地镜像而不是整棵树失败：侧栏是主界面的一部分，旧版本后台没有这个
     * 接口时它必须照常渲染；写操作仍然会如实报错，不会静默变成「点了没反应」。
     */
    let facts = crate::session_menu::list(None, true).unwrap_or_default();

    /*
     * 本地待激活记录按标识索引一份：服务端已经看得见的那条只用来补字段。
     *
     * 会话表面的渠道与回复目标取自首条入站，因此刚登记、还没有消息的会话在服务端
     * 这两项是空的；本地记录里的那一份正是它此刻唯一的来源，提交首条消息要用它。
     */
    let supplements: HashMap<&str, &crate::state::PendingSession> = state
        .pending_sessions
        .iter()
        .map(|pending| (pending.session_id.as_str(), pending))
        .collect();

    // 车道先按 workspace_id 归堆；未登记工作区的车道单独兜底，绝不静默丢弃。
    let mut buckets: HashMap<String, Vec<SessionDto>> = HashMap::new();
    let mut observed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for session in source.sessions()? {
        let workspace_id = if session.workspace_id.is_empty() {
            String::new()
        } else {
            session.workspace_id.clone()
        };
        // 归档位先取：标题、渠道与目标下面会逐个被搬走，之后再借整条会话是借不到的。
        let archived = archived_of(&session, &state.session_archived);
        let title = state
            .session_title
            .get(&session.session_id)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or(session.title);
        // 服务端还没给出标题时回落占位文案：本地只保存用户重命名，
        // 一旦把「新会话」写进本地，就会永久盖住服务端随对话生成的真实标题。
        let title = if title.trim().is_empty() {
            "新会话".to_string()
        } else {
            title
        };
        observed.insert(session.session_id.clone());
        let supplement = supplements.get(session.session_id.as_str()).copied();
        let channel = if session.channel.trim().is_empty() {
            DESKTOP_SESSION_CHANNEL.to_string()
        } else {
            session.channel
        };
        let destination = if session.destination.trim().is_empty() {
            supplement
                .map(|pending| pending.destination.clone())
                .unwrap_or_default()
        } else {
            session.destination
        };
        buckets.entry(workspace_id).or_default().push(SessionDto {
            pinned: state
                .session_pinned
                .iter()
                .any(|id| id == &session.session_id),
            archived,
            interaction_count: session.interaction_count,
            id: session.session_id,
            title,
            channel,
            destination,
            audience: session.audience,
            // 待答计数由服务端按库里的问答记录聚合；字段缺失时按无待答处理。
            pending_question_count: session.pending_question_count,
            created_at: session.created_at_ms,
            updated_at: session.updated_at_ms,
            // 「待激活」按内容判定：还没有任何一轮的会话都是尚未开始的会话，
            // 与它是否已在服务端登记无关。侧栏据此只留当前那一条，空会话不再长期占位。
            pending: session.interaction_count == 0,
            draft: false,
            running: session.running,
        });
    }

    // 新建但尚未产生 Interaction 的车道也要可见，否则用户会以为没建成功。
    for pending in &state.pending_sessions {
        // 已经产生 Interaction 的待激活车道会同时出现在真实车道列表里，
        // 同一条车道只允许出现一次，否则侧栏会出现两张指向同一目标的卡片。
        if observed.contains(&pending.session_id) {
            continue;
        }
        let title = state
            .session_title
            .get(&pending.session_id)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "新会话".to_string());
        buckets
            .entry(pending.workspace_id.clone())
            .or_default()
            .push(SessionDto {
                pinned: false,
                archived: false,
                id: pending.session_id.clone(),
                title,
                channel: DESKTOP_SESSION_CHANNEL.to_string(),
                destination: pending.destination.clone(),
                audience: "私密".to_string(),
                interaction_count: 0,
                pending_question_count: 0,
                created_at: pending.created_at,
                updated_at: pending.created_at,
                pending: true,
                draft: is_draft_id(&pending.session_id),
                running: false,
            });
    }

    // 未被用户添加的工作区下的车道一律不展示，因此桶里剩余的分组整体丢弃。
    let workspaces: Vec<WorkspaceDto> = entries
        .into_iter()
        .map(|entry| {
            let mut sessions = buckets.remove(&entry.id).unwrap_or_default();
            apply_session_facts(&mut sessions, &facts, &state.session_title);
            order_sessions(
                &mut sessions,
                &state.session_pinned,
                state.session_order.get(&entry.id),
            );
            WorkspaceDto {
                id: entry.id,
                name: entry.name,
                path: entry.path,
                active: entry.active,
                removable: entry.removable,
                sessions,
            }
        })
        .collect();

    Ok(workspaces)
}

/// 组合根入口：在给定数据源上组装工作区与车道列表。
pub fn workspace_list_at(source: &dyn SessionSource) -> Result<Vec<WorkspaceDto>, String> {
    build_workspaces(source)
}
