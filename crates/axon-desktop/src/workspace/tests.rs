//! 移除范围的单元测试。
//!
//! 回归的正是「移除工作区没效果」：界面给的是运行态工作区标识，而本地登记项
//! 以路径为身份，只比较其中一个身份就会静默失配。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::model::{SessionDto, normalize};
use super::mutation::RemovalScope;
use super::projection::apply_session_facts;
use crate::session_menu::SessionMenuEntry;
use crate::session_source::WorkspaceEntry as RegistryWorkspace;
use crate::state::{DesktopState, PendingSession, RegisteredWorkspace, directory_label};

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
    desktop.pending_sessions.push(PendingSession {
        session_id: "session-session-target".to_string(),
        workspace_id: "workspace-target".to_string(),
        destination: "workbench-1".to_string(),
        created_at: 1,
    });
    desktop.session_order.insert(
        "workspace-target".to_string(),
        vec!["session-session-target".to_string()],
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
        desktop.pending_sessions.is_empty(),
        "被移除工作区的待激活车道不得留下：重新添加同一目录会冒出一条空会话"
    );
    assert!(
        !desktop.session_order.contains_key("workspace-target"),
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
    // 前缀按当前平台的分隔符原样拼：unix 上再把分隔符换成反斜杠，等于让「归一化」多背一层
    // 分隔符差异，本用例只验证扩展前缀不影响身份比较。
    let extended = format!(r"\\?\{}", target.to_string_lossy());
    assert_eq!(
        normalize(Path::new(&extended)),
        normalize(&target),
        "同一目录的两种写法必须归一化到同一个字符串"
    );
}

/// 造一条车道；参数只给与本次判定相关的字段。
fn session_dto(id: &str, title: &str, pinned: bool, archived: bool, updated_at: u64) -> SessionDto {
    SessionDto {
        id: id.to_string(),
        title: title.to_string(),
        channel: "desktop".to_string(),
        destination: format!("workbench-{id}"),
        audience: "私密".to_string(),
        interaction_count: 0,
        pending_question_count: 0,
        created_at: 1,
        updated_at,
        pinned,
        archived,
        pending: false,
        running: false,
    }
}

/// 造一条运行态会话条目。
fn session_fact(
    id: &str,
    title: &str,
    pinned: bool,
    archived: bool,
    last_activity: u64,
) -> SessionMenuEntry {
    SessionMenuEntry {
        session_id: id.to_string(),
        title: title.to_string(),
        status: "completed".to_string(),
        pinned,
        archived,
        workspace_id: Some("workspace-target".to_string()),
        last_activity_at_ms: last_activity,
        turn_count: 2,
        total_active_ms: 0,
        ttft_p50_ms: None,
        ttft_max_ms: 0,
        open_job_count: 0,
    }
}

#[test]
fn runtime_session_facts_win_over_the_local_mirror() {
    // 置顶、归档与最近活动时间都以 `sessions` 表为准：本地镜像只是旧版本后台或缺字段
    // 时的兜底，不能让「桌面自己存了一份状态」盖住三端共用的那一份。
    let mut sessions = vec![session_dto("session-session-a", "旧标题", false, false, 10)];
    let facts = vec![session_fact("session-session-a", "新标题", true, true, 900)];
    apply_session_facts(&mut sessions, &facts, &HashMap::new());

    assert!(sessions[0].pinned, "置顶必须取会话表的值");
    assert!(sessions[0].archived, "归档必须取会话表的值");
    assert_eq!(sessions[0].title, "新标题", "标题取会话表给出的结论");
    assert_eq!(
        sessions[0].updated_at, 900,
        "活动时刻对齐后，按它排序就是网关那条「最近活动倒序」"
    );
}

#[test]
fn a_local_rename_still_wins_over_the_generated_title() {
    // 会话标题是网关按首条入站消息给出的；用户在本机改过的名字是本地偏好，优先。
    // 投影在进入这一步之前已经把本地名字写进车道标题，因此这里只负责「不覆盖」。
    let mut sessions = vec![session_dto(
        "session-session-a",
        "我改的名字",
        false,
        false,
        10,
    )];
    let facts = vec![session_fact(
        "session-session-a",
        "网关标题",
        false,
        false,
        900,
    )];
    let mut renamed = HashMap::new();
    renamed.insert("session-session-a".to_string(), "我改的名字".to_string());

    apply_session_facts(&mut sessions, &facts, &renamed);
    assert_eq!(sessions[0].title, "我改的名字");
    // 状态仍然以运行态为准：本地重命名只影响标题。
    assert_eq!(sessions[0].updated_at, 900);
}

#[test]
fn a_session_without_a_runtime_session_keeps_its_local_state() {
    // 本端刚新建、还没有首条消息的会话不在清单里：此时没有可对齐的事实，
    // 不能把用户刚点下的置顶抹掉，也不能把时间清成零。
    let mut sessions = vec![session_dto("session-session-new", "新会话", true, false, 5)];
    let facts = vec![session_fact(
        "session-session-other",
        "别的会话",
        false,
        true,
        900,
    )];

    apply_session_facts(&mut sessions, &facts, &HashMap::new());
    assert!(sessions[0].pinned, "不在清单里的车道保持本地镜像");
    assert!(!sessions[0].archived);
    assert_eq!(sessions[0].updated_at, 5);
    assert_eq!(sessions[0].title, "新会话");
}
