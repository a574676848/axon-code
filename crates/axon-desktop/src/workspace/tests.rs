//! 移除范围的单元测试。
//!
//! 回归的正是「移除工作区没效果」：界面给的是运行态工作区标识，而本地登记项
//! 以路径为身份，只比较其中一个身份就会静默失配。

use std::path::{Path, PathBuf};

use super::model::normalize;
use super::mutation::RemovalScope;
use crate::lane_source::WorkspaceEntry as RegistryWorkspace;
use crate::state::{DesktopState, PendingLane, RegisteredWorkspace, directory_label};

/// 造一条运行态工作区记录。
fn record(workspace_id: &str, root: &Path) -> RegistryWorkspace {
    RegistryWorkspace {
        workspace_id: workspace_id.to_string(),
        display_name: directory_label(&root.to_string_lossy()),
        workspace_root: root.to_string_lossy().to_string(),
        git_remote: None,
        default_branch: None,
        pinned: false,
        last_seen_at_ms: 0,
    }
}

/// 造一份登记了两个目录的本地状态。
fn state_with_two_workspaces() -> (DesktopState, PathBuf, PathBuf) {
    let parent = PathBuf::from("work");
    let target = parent.join("target");
    let sibling = parent.join("target-sibling");
    let mut desktop = DesktopState {
        workspaces: vec![
            RegisteredWorkspace {
                path: target.to_string_lossy().to_string(),
                name: "target".to_string(),
            },
            RegisteredWorkspace {
                path: sibling.to_string_lossy().to_string(),
                name: "target-sibling".to_string(),
            },
        ],
        ..DesktopState::default()
    };
    desktop.pending_lanes.push(PendingLane {
        lane_id: "session-lane-target".to_string(),
        workspace_id: "workspace-target".to_string(),
        destination: "workbench-1".to_string(),
        created_at: 1,
    });
    desktop.lane_order.insert(
        "workspace-target".to_string(),
        vec!["session-lane-target".to_string()],
    );
    (desktop, target, sibling)
}

#[test]
fn removal_scope_resolves_the_interface_identity_into_the_registered_path() {
    // 界面拿到的是运行态工作区标识，而本地登记项以路径为身份；
    // 只按其中一个身份比较正是「移除工作区没效果」的根因。
    let (desktop, target, sibling) = state_with_two_workspaces();
    let records = vec![record("workspace-target", &target)];
    let scope = RemovalScope::resolve("workspace-target", &records);
    let mut desktop = desktop;

    assert!(scope.apply(&mut desktop), "按标识移除必须真的改动状态");
    let remaining: Vec<String> = desktop
        .workspaces
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    assert_eq!(
        remaining,
        vec![sibling.to_string_lossy().to_string()],
        "只允许移除目标目录，路径相近的兄弟工作区必须原样保留"
    );
}

#[test]
fn removal_by_path_matches_the_same_entry_and_still_clears_derived_records() {
    let (desktop, target, _) = state_with_two_workspaces();
    let records = vec![record("workspace-target", &target)];
    // 路径身份同样要能命中：调用方可能只持有登记路径。
    let scope = RemovalScope::resolve(&target.to_string_lossy(), &records);
    let mut desktop = desktop;

    assert!(scope.apply(&mut desktop));
    assert_eq!(desktop.workspaces.len(), 1);
    assert!(
        desktop.pending_lanes.is_empty(),
        "被移除工作区的待激活车道不得留下：重新添加同一目录会冒出一条空会话"
    );
    assert!(
        !desktop.lane_order.contains_key("workspace-target"),
        "被移除工作区的手动顺序必须一并清掉"
    );
}

#[test]
fn unrelated_identity_changes_nothing() {
    let (desktop, target, _) = state_with_two_workspaces();
    let records = vec![record("workspace-target", &target)];
    let scope = RemovalScope::resolve("workspace-other", &records);
    let mut desktop = desktop;

    // 未命中时返回 false：调用方据此如实报错，而不是假装移除成功。
    assert!(!scope.apply(&mut desktop));
    assert_eq!(desktop.workspaces.len(), 2);
}

#[test]
fn registry_root_with_extended_prefix_matches_the_registered_path() {
    // 运行态注册表的根目录在 Windows 上带扩展长度前缀，本地登记项是普通路径；
    // 两者归一化之后必须相等，否则按标识移除仍然会落空。
    let target = PathBuf::from("work").join("target");
    let extended = format!(r"\\?\{}", target.to_string_lossy().replace('/', r"\"));
    assert_eq!(
        normalize(Path::new(&extended)),
        normalize(&target),
        "同一目录的两种写法必须归一化到同一个字符串"
    );
}
