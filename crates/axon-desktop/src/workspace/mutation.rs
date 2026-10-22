//! 工作区与车道的写入路径：登记、移除与车道维护。
//!
//! 每一次写入都是「先动服务端、再落本地状态、最后重新投影」：本地状态里只保存
//! 用户自己的选择（登记项、置顶、归档、重命名、手动顺序），因此写入永远以
//! 运行态的真实结论为前提，不在本地造第二份事实。

use std::path::{Path, PathBuf};

use crate::lane_source::{DESKTOP_LANE_CHANNEL, LaneSource, WorkspaceEntry as RegistryWorkspace};
use crate::state::{DesktopState, RegisteredWorkspace, directory_label, load_state, save_state};

use super::model::{WorkspaceDto, generate_uuid, normalize, now_millis};
use super::projection::build_workspaces;

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
            desktop.pending_lanes.len(),
            desktop.lane_order.len(),
        );
        desktop
            .workspaces
            .retain(|existing| !self.paths.contains(&normalize(Path::new(&existing.path))));
        // 待激活车道与手动顺序都属于被移除的工作区：留下它们会在重新添加同一
        // 目录时冒出一条谁也不认识的空会话。
        desktop
            .pending_lanes
            .retain(|lane| !self.ids.contains(&lane.workspace_id));
        desktop
            .lane_order
            .retain(|workspace_id, _| !self.ids.contains(workspace_id));
        before
            != (
                desktop.workspaces.len(),
                desktop.pending_lanes.len(),
                desktop.lane_order.len(),
            )
    }
}

/// 在给定数据源上登记一个工作区目录。
pub fn execute_workspace_add(
    source: &dyn LaneSource,
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
    source: &dyn LaneSource,
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

/// 在给定数据源上新建一条待激活车道。
///
/// 车道的真实活动由运行时的 Interaction 产生，因此这里登记的是
/// 本应用渠道下的待激活车道：身份立即确定并可见，首个 Interaction
/// 落地后即转为活跃车道。
///
/// **同一工作区已有待激活车道时直接复用**，不再新建第二条：待激活车道是
/// 「尚未开始的会话」，同一时刻每个工作区只需要一条；允许堆积只会留下一串
/// 既没有内容、也没有意义可言的空会话。
pub fn execute_lane_create(
    source: &dyn LaneSource,
    workspace_id: &str,
) -> Result<Vec<WorkspaceDto>, String> {
    // 只有「服务端还看不见」的待激活车道才值得复用；已经产生 Interaction 的车道
    // 早就是真实车道了，再拿它当新建目标会让「新会话」什么都不做。
    let observed: std::collections::HashSet<String> = source
        .lanes()?
        .into_iter()
        .map(|lane| lane.lane_id)
        .collect();
    // 复用判断先按当前快照早退一次：能复用就不必再向服务端派生车道标识。
    let current = load_state();
    let reusable = |desktop: &crate::state::DesktopState| {
        desktop
            .pending_lanes
            .iter()
            .any(|lane| lane.workspace_id == workspace_id && !observed.contains(&lane.lane_id))
    };
    if reusable(&current) {
        return build_workspaces(source);
    }
    let destination = format!("workbench-{}", generate_uuid());
    // 车道标识在服务端派生，本地不复刻哈希算法。
    let lane_id = source.derive_lane_id(workspace_id, DESKTOP_LANE_CHANNEL, &destination)?;

    /*
     * 写入前重新读取本地状态。
     *
     * 「读 → 网络 → 写」会留下一个丢失更新的窗口：窗口里来自别处的状态变更
     * （另一个窗口、命令行工具、用户直接编辑）都会被这次写入静默覆盖掉
     * （实测过：恢复桌面状态的同时新建会话，会把刚恢复的内容冲掉）。
     * 因此真正要写的那份状态必须在所有网络往返之后取，并在它上面再判一次复用。
     */
    let mut desktop = load_state();
    if reusable(&desktop) {
        return build_workspaces(source);
    }
    // 顺手清掉已落地为真实车道的本地待激活记录，避免越积越多。
    desktop
        .pending_lanes
        .retain(|lane| !observed.contains(&lane.lane_id));

    desktop.pending_lanes.retain(|lane| lane.lane_id != lane_id);
    desktop.pending_lanes.push(crate::state::PendingLane {
        lane_id: lane_id.clone(),
        workspace_id: workspace_id.to_string(),
        destination,
        created_at: now_millis(),
    });
    // 本地只保存用户重命名；未命名车道的占位文案在构建时回落。
    save_state(&desktop)?;

    build_workspaces(source)
}

/// 在给定数据源上设置或取消车道置顶。
pub fn execute_lane_set_pinned(
    source: &dyn LaneSource,
    lane_id: &str,
    pinned: bool,
) -> Result<Vec<WorkspaceDto>, String> {
    let mut desktop = load_state();
    desktop.lane_pinned.retain(|id| id != lane_id);
    if pinned {
        desktop.lane_pinned.push(lane_id.to_string());
    }
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 在给定数据源上设置或取消车道归档。
pub fn execute_lane_set_archived(
    source: &dyn LaneSource,
    lane_id: &str,
    archived: bool,
) -> Result<Vec<WorkspaceDto>, String> {
    let mut desktop = load_state();
    desktop.lane_archived.retain(|id| id != lane_id);
    if archived {
        desktop.lane_archived.push(lane_id.to_string());
    }
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 在给定数据源上重命名车道。
pub fn execute_lane_rename(
    source: &dyn LaneSource,
    lane_id: &str,
    title: &str,
) -> Result<Vec<WorkspaceDto>, String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err("车道标题不能为空".to_string());
    }
    let mut desktop = load_state();
    desktop
        .lane_title
        .insert(lane_id.to_string(), trimmed.to_string());
    save_state(&desktop)?;
    build_workspaces(source)
}

/// 在给定数据源上保存某个工作区下车道的自定义顺序。
pub fn execute_lane_reorder(
    source: &dyn LaneSource,
    workspace_id: &str,
    ordered_ids: Vec<String>,
) -> Result<Vec<WorkspaceDto>, String> {
    if ordered_ids.is_empty() {
        return Err("排序结果不能为空".to_string());
    }
    let mut desktop = load_state();
    desktop
        .lane_order
        .insert(workspace_id.to_string(), ordered_ids);
    save_state(&desktop)?;
    build_workspaces(source)
}
