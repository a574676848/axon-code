//! 工作区与车道的只读投影：把真实运行态整理成界面需要的形状。
//!
//! 这一层只读：它读本地登记状态、读运行态注册表与车道清单，输出展示模型。
//! 任何写入都在 [`super::mutation`] 里完成，因此这里不存在第二份可写副本。

use std::collections::HashMap;
use std::path::Path;

use crate::lane_source::{DESKTOP_LANE_CHANNEL, LaneSource};
use crate::state::{directory_label, load_state};

use super::model::{LaneDto, WorkspaceDto, normalize, runtime_workspace_dir};

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
fn order_lanes(lanes: &mut [LaneDto], pinned: &[String], manual: Option<&Vec<String>>) {
    let rank = |id: &str| -> usize {
        pinned
            .iter()
            .position(|entry| entry == id)
            .unwrap_or(usize::MAX)
    };
    lanes.sort_by(|left, right| {
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
fn workspace_entries(source: &dyn LaneSource) -> Result<Vec<WorkspaceEntry>, String> {
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

/// 组装全部工作区及其车道。
pub(super) fn build_workspaces(source: &dyn LaneSource) -> Result<Vec<WorkspaceDto>, String> {
    let state = load_state();
    let entries = workspace_entries(source)?;

    // 车道先按 workspace_id 归堆；未登记工作区的车道单独兜底，绝不静默丢弃。
    let mut buckets: HashMap<String, Vec<LaneDto>> = HashMap::new();
    let mut observed: std::collections::HashSet<String> = std::collections::HashSet::new();
    for lane in source.lanes()? {
        let workspace_id = if lane.workspace_id.is_empty() {
            String::new()
        } else {
            lane.workspace_id.clone()
        };
        let title = state
            .lane_title
            .get(&lane.lane_id)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or(lane.title);
        // 服务端还没给出标题时回落占位文案：本地只保存用户重命名，
        // 一旦把「新会话」写进本地，就会永久盖住服务端随对话生成的真实标题。
        let title = if title.trim().is_empty() {
            "新会话".to_string()
        } else {
            title
        };
        observed.insert(lane.lane_id.clone());
        buckets.entry(workspace_id).or_default().push(LaneDto {
            pinned: state.lane_pinned.iter().any(|id| id == &lane.lane_id),
            archived: state.lane_archived.iter().any(|id| id == &lane.lane_id),
            id: lane.lane_id,
            title,
            channel: lane.channel,
            destination: lane.destination,
            audience: lane.audience,
            interaction_count: lane.interaction_count,
            // 待答计数由服务端按库里的问答记录聚合；字段缺失时按无待答处理。
            pending_question_count: lane.pending_question_count,
            created_at: lane.created_at_ms,
            updated_at: lane.updated_at_ms,
            pending: false,
            running: lane.running,
        });
    }

    // 新建但尚未产生 Interaction 的车道也要可见，否则用户会以为没建成功。
    for pending in &state.pending_lanes {
        // 已经产生 Interaction 的待激活车道会同时出现在真实车道列表里，
        // 同一条车道只允许出现一次，否则侧栏会出现两张指向同一目标的卡片。
        if observed.contains(&pending.lane_id) {
            continue;
        }
        let title = state
            .lane_title
            .get(&pending.lane_id)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "新会话".to_string());
        buckets
            .entry(pending.workspace_id.clone())
            .or_default()
            .push(LaneDto {
                pinned: false,
                archived: false,
                id: pending.lane_id.clone(),
                title,
                channel: DESKTOP_LANE_CHANNEL.to_string(),
                destination: pending.destination.clone(),
                audience: "私密".to_string(),
                interaction_count: 0,
                pending_question_count: 0,
                created_at: pending.created_at,
                updated_at: pending.created_at,
                pending: true,
                running: false,
            });
    }

    // 未被用户添加的工作区下的车道一律不展示，因此桶里剩余的分组整体丢弃。
    let workspaces: Vec<WorkspaceDto> = entries
        .into_iter()
        .map(|entry| {
            let mut lanes = buckets.remove(&entry.id).unwrap_or_default();
            order_lanes(
                &mut lanes,
                &state.lane_pinned,
                state.lane_order.get(&entry.id),
            );
            WorkspaceDto {
                id: entry.id,
                name: entry.name,
                path: entry.path,
                active: entry.active,
                removable: entry.removable,
                lanes,
            }
        })
        .collect();

    Ok(workspaces)
}

/// 组合根入口：在给定数据源上组装工作区与车道列表。
pub fn workspace_list_at(source: &dyn LaneSource) -> Result<Vec<WorkspaceDto>, String> {
    build_workspaces(source)
}
