//! 子代理会话、目标执行会话与定时任务会话的解析测试：标识是 ULID，关系只落在列上。

use super::SqliteRuntimeStore;
use crate::SessionKind;
use crate::ports::{
    GoalSessionRequest, NewSessionRequest, RuntimeStore, ScheduleSessionRequest,
    SubagentSessionRequest, WorkspaceStore,
};
use axon_domain::DaemonSessionId;

/// 根会话标识：生产侧由 `new_session_id()` 生成，形态固定为 ULID。
const ROOT_SESSION: &str = "01J8XQ7K3M4N5P6Q7R8S9T0V1W";
const ROOT_A: &str = "01J8XQ7K3M4N5P6Q7R8S9T0V2W";
const ROOT_B: &str = "01J8XQ7K3M4N5P6Q7R8S9T0V3W";

fn store() -> (axon_test_support::TestTempDir, SqliteRuntimeStore) {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    (directory, store)
}

fn root_session(store: &SqliteRuntimeStore, session_id: &str) -> DaemonSessionId {
    let id = DaemonSessionId::new(session_id).expect("session");
    store
        .create_session_with(&NewSessionRequest {
            workspace_id: Some("workspace-a".to_owned()),
            ..NewSessionRequest::primary(id.clone(), "E:/repo", 10)
        })
        .expect("根会话");
    id
}

#[test]
fn subagent_session_is_a_ulid_with_root_and_agent_path() {
    let (_directory, store) = store();
    let root = root_session(&store, ROOT_SESSION);
    let resolved = store
        .resolve_subagent_session(&SubagentSessionRequest {
            root_session_id: root.clone(),
            agent_path: "/root/delegate".to_owned(),
            cwd: "E:/repo".to_owned(),
            now_ms: 20,
        })
        .expect("解析子代理会话");
    assert_eq!(resolved.kind, SessionKind::Subagent);
    assert_eq!(resolved.root_session_id.as_ref(), Some(&root));
    assert_eq!(resolved.agent_path.as_deref(), Some("root/delegate"));
    // Workspace 从根会话继承：子代理与父会话属于同一个工作区。
    assert_eq!(resolved.workspace_id.as_deref(), Some("workspace-a"));
    // 标识本身不带任何路径或前缀。
    assert!(
        crate::is_ulid(resolved.session_id.as_str()),
        "子代理会话标识必须是 ULID：{}",
        resolved.session_id.as_str()
    );
    assert!(!resolved.session_id.as_str().contains("subagent"));
}

#[test]
fn resolving_the_same_agent_path_returns_the_same_session() {
    let (_directory, store) = store();
    let root = root_session(&store, ROOT_SESSION);
    let request = SubagentSessionRequest {
        root_session_id: root.clone(),
        agent_path: "root/delegate".to_owned(),
        cwd: "E:/repo".to_owned(),
        now_ms: 20,
    };
    let first = store.resolve_subagent_session(&request).expect("首次");
    // 路径只差开头斜杠也是同一个子代理：入参先归一，再按唯一键认人。
    let second = store
        .resolve_subagent_session(&SubagentSessionRequest {
            agent_path: "/root/delegate".to_owned(),
            ..request
        })
        .expect("再次");
    assert_eq!(first.session_id, second.session_id);
}

#[test]
fn different_roots_or_paths_get_different_sessions() {
    let (_directory, store) = store();
    let root_a = root_session(&store, ROOT_A);
    let root_b = root_session(&store, ROOT_B);
    let base = SubagentSessionRequest {
        root_session_id: root_a.clone(),
        agent_path: "root/delegate".to_owned(),
        cwd: "E:/repo".to_owned(),
        now_ms: 20,
    };
    let first = store.resolve_subagent_session(&base).expect("A");
    let sibling = store
        .resolve_subagent_session(&SubagentSessionRequest {
            agent_path: "root/reviewer".to_owned(),
            ..base.clone()
        })
        .expect("同根另一个代理");
    let other_root = store
        .resolve_subagent_session(&SubagentSessionRequest {
            root_session_id: root_b,
            ..base
        })
        .expect("另一个根会话");
    assert_ne!(first.session_id, sibling.session_id);
    assert_ne!(first.session_id, other_root.session_id);
}

#[test]
fn find_subagent_session_does_not_create_anything() {
    let (_directory, store) = store();
    let root = root_session(&store, ROOT_SESSION);
    assert!(
        store
            .find_subagent_session(&root, "root/missing")
            .expect("查询成功")
            .is_none()
    );
    // 只读查询不建行：名册与唤醒前置校验都靠它，不能在查询路径上产生会话。
    let subagents: i64 = store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM sessions WHERE kind='subagent'",
                [],
                |row| row.get(0),
            )
        })
        .expect("计数");
    assert_eq!(subagents, 0);
}

/// 根会话列只收会话 ULID：Thread 标识是 36 位 UUID，混进来会让子代理永远找不到父会话。
#[test]
fn subagent_root_rejects_thread_uuid_instead_of_mixing_identity_spaces() {
    let (_directory, store) = store();
    let thread_id =
        DaemonSessionId::new("5f0b8c1e-2a3d-4e5f-8a9b-0c1d2e3f4a5b".to_owned()).expect("非空");
    let error = store
        .resolve_subagent_session(&SubagentSessionRequest {
            root_session_id: thread_id.clone(),
            agent_path: "root/delegate".to_owned(),
            cwd: "E:/repo".to_owned(),
            now_ms: 20,
        })
        .expect_err("Thread 标识不得充当根会话");
    assert!(error.to_string().contains("ULID"), "{error}");
    // 拒绝发生在唯一写入口：库里不会留下这条既找不到父会话、又占着唯一键的行。
    assert!(
        store
            .find_subagent_session(&thread_id, "root/delegate")
            .expect("查询成功")
            .is_none()
    );
}

#[test]
fn goal_session_is_a_ulid_and_reused_per_goal() {
    let (_directory, store) = store();
    let first = store
        .resolve_goal_session(&GoalSessionRequest {
            goal_id: "goal-a".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            cwd: "E:/repo".to_owned(),
            now_ms: 30,
        })
        .expect("首次解析");
    let second = store
        .resolve_goal_session(&GoalSessionRequest {
            goal_id: "goal-a".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            cwd: "E:/repo".to_owned(),
            now_ms: 40,
        })
        .expect("再次解析");
    assert_eq!(first.session_id, second.session_id);
    assert_eq!(first.kind, SessionKind::Goal);
    assert!(crate::is_ulid(first.session_id.as_str()));
    assert_ne!(
        store
            .resolve_goal_session(&GoalSessionRequest {
                goal_id: "goal-b".to_owned(),
                workspace_id: None,
                cwd: "E:/repo".to_owned(),
                now_ms: 50,
            })
            .expect("另一个目标")
            .session_id,
        first.session_id
    );
}

/// 登记一个 Workspace：定时任务会话从它继承工作区与根目录。
fn register_workspace(store: &SqliteRuntimeStore) {
    store
        .register_workspace(&crate::WorkspaceRecord {
            workspace_id: "workspace-a".to_owned(),
            display_name: "workspace-a".to_owned(),
            workspace_root: "E:/work".to_owned(),
            git_remote: None,
            default_branch: None,
            aliases: Vec::new(),
            memory_namespace: "workspace-a".to_owned(),
            pinned: false,
            last_seen_at_ms: 10,
        })
        .expect("登记 Workspace");
}

#[test]
fn schedule_session_is_a_ulid_cron_session_with_workspace_binding() {
    let (_directory, store) = store();
    register_workspace(&store);
    let resolved = store
        .resolve_schedule_session(&ScheduleSessionRequest {
            schedule_id: "heartbeat-a".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            now_ms: 60,
        })
        .expect("解析定时任务会话");
    assert_eq!(resolved.kind, SessionKind::Cron);
    assert!(crate::is_ulid(resolved.session_id.as_str()));
    assert_eq!(resolved.workspace_id.as_deref(), Some("workspace-a"));
    assert_eq!(resolved.cwd, "E:/work");
}

#[test]
fn bound_schedule_session_is_reused_instead_of_creating_another() {
    let (_directory, store) = store();
    register_workspace(&store);
    let first = store
        .resolve_schedule_session(&ScheduleSessionRequest {
            schedule_id: "heartbeat-a".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            now_ms: 60,
        })
        .expect("首次解析");
    // 定义侧把会话写进调度记录，这就是绑定的事实源。
    store
        .upsert_schedule(&crate::RuntimeScheduleRecord {
            schedule_id: "heartbeat-a".to_owned(),
            session_id: Some(first.session_id.clone()),
            kind: "heartbeat".to_owned(),
            expression: "interval_ms:60000".to_owned(),
            payload_json: "{}".to_owned(),
            delivery_mode: "follow_up".to_owned(),
            status: "active".to_owned(),
            next_run_at_ms: Some(120),
            lease_owner: None,
            lease_expires_at_ms: None,
            failure_count: 0,
        })
        .expect("保存调度");

    let second = store
        .resolve_schedule_session(&ScheduleSessionRequest {
            schedule_id: "heartbeat-a".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            now_ms: 70,
        })
        .expect("再次解析");
    assert_eq!(second.session_id, first.session_id);
    // 另一个任务拿到的是另一条会话。
    let other = store
        .resolve_schedule_session(&ScheduleSessionRequest {
            schedule_id: "heartbeat-b".to_owned(),
            workspace_id: Some("workspace-a".to_owned()),
            now_ms: 80,
        })
        .expect("另一个任务");
    assert_ne!(other.session_id, first.session_id);
}
