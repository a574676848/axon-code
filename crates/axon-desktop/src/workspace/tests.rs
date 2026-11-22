//! 移除范围的单元测试。
//!
//! 回归的正是「移除工作区没效果」：界面给的是运行态工作区标识，而本地登记项
//! 以路径为身份，只比较其中一个身份就会静默失配。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::model::{SessionDto, draft_id, is_draft_id, normalize};
use super::mutation::{
    RemovalScope, execute_session_rename, prune_pending, rollback_gateway_title,
};
use super::projection::{apply_session_facts, archived_of};
use crate::session_menu::SessionMenuEntry;
use crate::session_source::WorkspaceEntry as RegistryWorkspace;
use crate::session_source::{SessionEntry, SessionSource};
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
        draft: false,
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

/// 造一条运行态会话条目；只给与清理判定相关的字段。
fn session_entry(id: &str, interaction_count: u64) -> SessionEntry {
    SessionEntry {
        session_id: id.to_string(),
        workspace_id: "workspace-target".to_string(),
        channel: "desktop".to_string(),
        destination: "workbench-1".to_string(),
        audience: "私密".to_string(),
        archived: false,
        title: String::new(),
        interaction_count,
        pending_question_count: 0,
        running: false,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

#[test]
fn the_archived_flag_comes_from_the_runtime_list() {
    // 归档可能发生在命令行、TUI 或另一个窗口：那一路的事实只落在会话表里，
    // 桌面本地镜像没有它。清单读全（`include_archived=1`）并带着这一位，
    // 「仅显示已归档」才看得到这些会话；只认本地镜像会读成一条都没有。
    let mut archived_in_runtime = session_entry("01M80SRNDN945AXEJTHF7WWH6T", 2);
    archived_in_runtime.archived = true;
    assert!(
        archived_of(&archived_in_runtime, &[]),
        "运行态说已归档，就必须读成已归档"
    );

    let active = session_entry("01M80TF0ER99D8RJNGERGJHHS7", 2);
    assert!(!archived_of(&active, &[]), "活跃会话不得被算成归档");

    // 旧版本本地镜像仍然兜底，且只作用于它自己那一条。
    let mirror = vec!["session-legacy".to_string()];
    assert!(
        archived_of(&session_entry("session-legacy", 0), &mirror),
        "本地镜像里的归档记录仍然要生效"
    );
    assert!(
        !archived_of(&active, &mirror),
        "镜像里别的会话不得牵连这一条"
    );
}

/// 造一条待激活记录。
fn pending_record(session_id: &str, created_at: u64) -> PendingSession {
    PendingSession {
        session_id: session_id.to_string(),
        workspace_id: "workspace-target".to_string(),
        destination: "workbench-1".to_string(),
        created_at,
    }
}

#[test]
fn a_draft_that_never_reached_the_server_expires_after_its_ttl() {
    // 草稿只要没提交过消息，服务端就永远不会有它：留着既不会被打开，
    // 也只会让状态文件越积越多。
    let ttl = 24 * 60 * 60 * 1_000;
    let mut desktop = DesktopState {
        pending_sessions: vec![
            pending_record("draft-old", 0),
            pending_record("draft-new", 100),
        ],
        ..DesktopState::default()
    };

    assert!(prune_pending(&mut desktop, &[], ttl + 50));
    assert_eq!(desktop.pending_sessions.len(), 1);
    assert_eq!(desktop.pending_sessions[0].session_id, "draft-new");
}

#[test]
fn a_fresh_draft_survives_the_cleanup() {
    let mut desktop = DesktopState {
        pending_sessions: vec![pending_record("draft-fresh", 1_000)],
        ..DesktopState::default()
    };

    assert!(!prune_pending(&mut desktop, &[], 2_000));
    assert_eq!(desktop.pending_sessions.len(), 1);
}

#[test]
fn a_materialized_session_keeps_its_local_target_until_it_has_content() {
    // 物化之后、首条消息之前：服务端已经看得见这条会话，但渠道与回复目标还是空的，
    // 本地记录是提交这一轮唯一的依据，因此不能清理。
    let mut desktop = DesktopState {
        pending_sessions: vec![pending_record("01M7QQ5VJQ", 0)],
        ..DesktopState::default()
    };

    assert!(!prune_pending(
        &mut desktop,
        &[session_entry("01M7QQ5VJQ", 0)],
        10_000_000
    ));
    assert_eq!(desktop.pending_sessions.len(), 1);

    // 有了内容之后，会话表面事实由服务端给出，本地镜像退场。
    assert!(prune_pending(
        &mut desktop,
        &[session_entry("01M7QQ5VJQ", 1)],
        10_000_000
    ));
    assert!(desktop.pending_sessions.is_empty());
}

#[test]
fn draft_ids_never_collide_with_server_identities() {
    let first = draft_id();
    assert!(is_draft_id(&first));
    assert_ne!(first, draft_id(), "每一条草稿都必须是独立标识");
    // 服务端标识是 26 位 ULID：既不带前缀，也不会被判成本机草稿。
    assert!(!is_draft_id("01M7RQ6K658M5BCYXFKJN3FM91"));
}

/// 重命名只在这一条用例里需要数据源：空标题在建任何请求之前就被拒绝，
/// 因此这里给一个「一调就失败」的桩，逼出「不该碰网关」这条断言。
struct UnreachableSource;

impl SessionSource for UnreachableSource {
    fn workspaces(&self) -> Result<Vec<RegistryWorkspace>, String> {
        Err("空标题不得走到网关".to_string())
    }

    fn sessions(&self) -> Result<Vec<SessionEntry>, String> {
        Err("空标题不得走到网关".to_string())
    }

    fn create_session(&self, _workspace_id: &str, _cwd: Option<&str>) -> Result<String, String> {
        Err("空标题不得走到网关".to_string())
    }

    fn ensure_workspace(&self, _root: &str) -> Result<RegistryWorkspace, String> {
        Err("空标题不得走到网关".to_string())
    }
}

#[test]
fn renaming_to_a_blank_title_is_rejected_before_any_write() {
    // 空标题会把车道退化成没有名字：本地镜像与网关都不该被碰。
    let error = execute_session_rename(&UnreachableSource, "session-a", "   ")
        .expect_err("空标题必须被拒绝");
    assert_eq!(error, "车道标题不能为空");
}

#[test]
fn local_mirror_failure_rolls_the_gateway_title_back() {
    // 网关已经写入新标题、本地镜像却落盘失败：把网关写回旧值，两端回到同一份事实。
    let mut patched = Vec::new();
    let message = rollback_gateway_title(
        Some("旧名字"),
        "新名字",
        "磁盘只读".to_string(),
        |title| {
            patched.push(title.to_string());
            Ok(())
        },
    );

    assert_eq!(
        patched,
        vec!["旧名字".to_string()],
        "回滚写的是改名前的标题"
    );
    assert!(message.contains("磁盘只读"), "{message}");
    assert!(message.contains("回滚为「旧名字」"), "{message}");
}

#[test]
fn rollback_failure_is_reported_instead_of_pretending_success() {
    let message = rollback_gateway_title(
        Some("旧名字"),
        "新名字",
        "磁盘只读".to_string(),
        |_| Err("网关不可达".to_string()),
    );

    assert!(message.contains("回滚失败"), "{message}");
    assert!(message.contains("网关不可达"), "{message}");
    assert!(message.contains("当前为「新名字」"), "{message}");
}

#[test]
fn a_first_rename_reports_that_the_gateway_keeps_the_new_title() {
    // 从未在本机改过名：网关标题来自首条入站消息，无法原样写回；
    // 此时如实说明「以网关为准」，而不是伪造一次回滚。
    let message = rollback_gateway_title(None, "新名字", "磁盘只读".to_string(), |_| {
        panic!("没有旧标题时不得发起回滚请求");
    });

    assert!(message.contains("以网关为准"), "{message}");
    assert!(message.contains("新名字"), "{message}");
}
