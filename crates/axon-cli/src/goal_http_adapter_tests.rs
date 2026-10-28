//! 目标看板路由测试：列表顺序与截断、作用域合并、非法参数与详情错误码。

use super::{GOALS_SUBPATH, GoalHttpAdapter};
use crate::workbench_http_adapter::{WORKBENCH_PATH_PREFIX, runtime_store};
use axon_daemon::GatewayHttpRouteHandler;
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, GoalPlan, GoalRecord, GoalStatus, ScopeRef,
};
use axon_session::{
    ExecutionStore, GoalStore, RuntimeStore, SqliteRuntimeStore, WorkspaceRecord, WorkspaceStore,
};
use serde_json::Value;

/// 构造一个指向临时运行态根的适配器与同源 Store。
fn fixture() -> (
    axon_test_support::TestTempDir,
    GoalHttpAdapter,
    SqliteRuntimeStore,
) {
    let directory = axon_test_support::test_temp_dir();
    let store = runtime_store(directory.path());
    // 先取路径再构造元组：目录句柄是所有者的最后一项，不能被提前移动。
    let adapter = GoalHttpAdapter::new(directory.path().to_path_buf());
    (directory, adapter, store)
}

fn goal(goal_id: &str, scope: ScopeRef, updated_at_ms: u64) -> GoalRecord {
    let mut record = GoalRecord::new(
        axon_domain::GoalId::new(goal_id).expect("goal id"),
        scope,
        format!("目标 {goal_id}"),
        GoalPlan {
            objective: format!("完成 {goal_id}"),
            steps: vec!["第一步".to_owned(), "第二步".to_owned()],
        },
        vec!["验收通过".to_owned()],
        updated_at_ms,
    )
    .expect("goal");
    record
        .transition(GoalStatus::Active, updated_at_ms)
        .expect("active");
    record.updated_at_ms = updated_at_ms;
    record
}

fn workspace(workspace_id: &str) -> WorkspaceRecord {
    WorkspaceRecord {
        workspace_id: workspace_id.to_owned(),
        display_name: format!("显示名 {workspace_id}"),
        workspace_root: format!("E:/{workspace_id}"),
        git_remote: None,
        default_branch: None,
        aliases: Vec::new(),
        memory_namespace: workspace_id.to_owned(),
        pinned: false,
        last_seen_at_ms: 1,
    }
}

fn list_path() -> String {
    format!("{WORKBENCH_PATH_PREFIX}/{GOALS_SUBPATH}")
}

fn detail_path(goal_id: &str) -> String {
    format!("{WORKBENCH_PATH_PREFIX}/{GOALS_SUBPATH}/{goal_id}")
}

fn get(adapter: &GoalHttpAdapter, path: &str, query: Option<&str>) -> (u16, Value) {
    let response = adapter
        .handle_request("GET", path, query, &[])
        .expect("路由命中");
    let body = serde_json::from_slice::<Value>(&response.body).expect("JSON 正文");
    (response.status, body)
}

#[test]
fn empty_ledger_returns_empty_list() {
    let (_directory, adapter, _store) = fixture();
    let (status, body) = get(&adapter, &list_path(), None);
    assert_eq!(status, 200);
    assert_eq!(body["goals"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["returned"], 0);
    assert_eq!(body["truncated"], false);
}

#[test]
fn list_merges_scopes_and_sorts_by_last_activity() {
    let (_directory, adapter, store) = fixture();
    store
        .register_workspace(&workspace("workspace-a"))
        .expect("workspace");
    // 三个目标的 updatedAtMs 是 100 / 300 / 200，但执行把最近活动改写成
    // 100 / 300 / 900。按 updatedAtMs 排序会得到 ws、newer、global；
    // 按 lastActivityAtMs 得到 newer、ws、global，两者相反，用例因此能分辨排序键。
    store
        .create_goal(&goal("goal-global", ScopeRef::global(), 100))
        .expect("goal");
    store
        .create_goal(&goal(
            "goal-ws",
            ScopeRef::workspace("workspace-a").expect("scope"),
            300,
        ))
        .expect("goal");
    store
        .create_goal(&goal("goal-newer", ScopeRef::global(), 200))
        .expect("goal");
    // 工作区目标的执行比它自己更旧：最近活动仍取目标自身的更新时间。
    bind_execution(&store, "goal-ws", 100);
    // 全局目标的执行最新：最近活动由执行抬到 900。
    bind_execution(&store, "goal-newer", 900);

    let (status, body) = get(&adapter, &list_path(), None);
    assert_eq!(status, 200);
    let ids: Vec<&str> = body["goals"]
        .as_array()
        .expect("goals")
        .iter()
        .map(|entry| entry["goalId"].as_str().expect("goalId"))
        .collect();
    // 最近活动降序：900 / 300 / 100。
    assert_eq!(ids, vec!["goal-newer", "goal-ws", "goal-global"]);
    assert_eq!(body["returned"], 3);
    assert_eq!(body["truncated"], false);

    // 全局目标的最新执行把最近活动推到 900，卡片同时带出执行读数。
    let most_active = &body["goals"][0];
    assert_eq!(most_active["goalId"], "goal-newer");
    assert_eq!(most_active["lastActivityAtMs"], 900);
    assert_eq!(most_active["latestExecution"]["updatedAtMs"], 900);

    let workspace_goal = body["goals"]
        .as_array()
        .expect("goals")
        .iter()
        .find(|entry| entry["goalId"] == "goal-ws")
        .expect("工作区目标");
    // 执行更旧，最近活动取目标自身的 300，而不是执行的 100。
    assert_eq!(workspace_goal["lastActivityAtMs"], 300);
    assert_eq!(workspace_goal["scopeKind"], "workspace");
    assert_eq!(workspace_goal["workspaceId"], "workspace-a");
    assert_eq!(workspace_goal["workspaceName"], "显示名 workspace-a");
    assert_eq!(workspace_goal["workspacePath"], "E:/workspace-a");
    assert_eq!(workspace_goal["stepCount"], 2);
    // 全局目标没有工作区事实：三段字段一律为空。
    let global_goal = body["goals"]
        .as_array()
        .expect("goals")
        .iter()
        .find(|entry| entry["goalId"] == "goal-global")
        .expect("全局目标");
    assert_eq!(global_goal["lastActivityAtMs"], 100);
    assert_eq!(global_goal["scopeKind"], "global");
    assert!(global_goal["workspaceId"].is_null());
    assert!(global_goal["workspaceName"].is_null());
    assert!(global_goal["workspacePath"].is_null());
}

/// 给目标挂一条已绑定会话的执行，用它改写该目标的最近活动时间。
fn bind_execution(store: &SqliteRuntimeStore, goal_id: &str, activity_at_ms: u64) {
    bind_execution_at(store, goal_id, activity_at_ms, "E:/workspace-a");
}

/// 同上，但显式给出会话 cwd：全局目标的工作区解析靠它匹配注册表根目录。
fn bind_execution_at(store: &SqliteRuntimeStore, goal_id: &str, activity_at_ms: u64, cwd: &str) {
    let session_id = DaemonSessionId::new(format!("goal-session-{goal_id}")).expect("session");
    store.create_session(&session_id, cwd, 10).expect("session");
    let mut execution = ExecutionRecord::new(
        ExecutionId::new(format!("execution-{goal_id}")).expect("execution id"),
        axon_domain::GoalId::new(goal_id).expect("goal id"),
        activity_at_ms,
    );
    execution.session_id = Some(session_id);
    execution.updated_at_ms = activity_at_ms;
    store.enqueue_execution(&execution).expect("execution");
}

#[test]
fn list_honors_limit_and_reports_truncation() {
    let (_directory, adapter, store) = fixture();
    for (index, updated_at_ms) in [100_u64, 200, 300].into_iter().enumerate() {
        store
            .create_goal(&goal(
                &format!("goal-{index}"),
                ScopeRef::global(),
                updated_at_ms,
            ))
            .expect("goal");
    }
    let (status, body) = get(&adapter, &list_path(), Some("limit=2"));
    assert_eq!(status, 200);
    assert_eq!(body["returned"], 2);
    assert_eq!(body["truncated"], true);
    assert_eq!(body["goals"][0]["goalId"], "goal-2");
}

#[test]
fn list_filters_by_workspace_and_rejects_bad_query() {
    let (_directory, adapter, store) = fixture();
    store
        .register_workspace(&workspace("workspace-a"))
        .expect("workspace");
    store
        .register_workspace(&workspace("workspace-b"))
        .expect("workspace");
    store
        .create_goal(&goal("goal-global", ScopeRef::global(), 100))
        .expect("goal");
    store
        .create_goal(&goal(
            "goal-a",
            ScopeRef::workspace("workspace-a").expect("scope"),
            200,
        ))
        .expect("goal");
    store
        .create_goal(&goal(
            "goal-b",
            ScopeRef::workspace("workspace-b").expect("scope"),
            300,
        ))
        .expect("goal");

    let (status, body) = get(&adapter, &list_path(), Some("workspaceId=workspace-a"));
    assert_eq!(status, 200);
    assert_eq!(body["returned"], 1);
    assert_eq!(body["goals"][0]["goalId"], "goal-a");

    // 未注册的工作区标识不属于可见集合：给出空结果，而不是把它当作可达作用域去查。
    let (status, body) = get(&adapter, &list_path(), Some("workspaceId=workspace-gone"));
    assert_eq!(status, 200);
    assert_eq!(body["returned"], 0);
    assert_eq!(body["goals"].as_array().map(Vec::len), Some(0));

    for query in ["limit=0", "limit=257", "limit=abc"] {
        let (status, body) = get(&adapter, &list_path(), Some(query));
        assert_eq!(status, 400, "{query}");
        assert_eq!(body["error"]["code"], "invalid_query", "{query}");
        assert_eq!(body["error"]["message"], "limit 必须在 1 到 256 之间");
        assert!(body["error"]["details"].is_null());
    }
}

#[test]
fn global_goal_resolves_workspace_from_its_execution_session() {
    let (_directory, adapter, store) = fixture();
    store
        .register_workspace(&workspace("workspace-a"))
        .expect("workspace");
    // 全局目标自身没有作用域工作区，但它的执行会话 cwd 唯一命中 workspace-a，
    // 因此三段工作区字段都要被回填，卡片才能进入会话。
    store
        .create_goal(&goal("goal-global", ScopeRef::global(), 100))
        .expect("goal");
    bind_execution(&store, "goal-global", 900);

    let (status, body) = get(&adapter, &detail_path("goal-global"), None);
    assert_eq!(status, 200);
    let card = &body["goal"];
    assert_eq!(card["scopeKind"], "global");
    assert_eq!(card["workspaceId"], "workspace-a");
    assert_eq!(card["workspaceName"], "显示名 workspace-a");
    assert_eq!(card["workspacePath"], "E:/workspace-a");
}

#[test]
fn global_goal_without_matching_cwd_keeps_workspace_empty() {
    let (_directory, adapter, store) = fixture();
    store
        .register_workspace(&workspace("workspace-a"))
        .expect("workspace");
    store
        .create_goal(&goal("goal-global", ScopeRef::global(), 100))
        .expect("goal");
    // 会话 cwd 不在注册表里：解析失败时三段字段一律为空，不猜一个最像的工作区。
    bind_execution_at(&store, "goal-global", 900, "E:/elsewhere");

    let (status, body) = get(&adapter, &detail_path("goal-global"), None);
    assert_eq!(status, 200);
    let card = &body["goal"];
    assert_eq!(card["scopeKind"], "global");
    assert!(card["workspaceId"].is_null());
    assert!(card["workspaceName"].is_null());
    assert!(card["workspacePath"].is_null());
}

#[test]
fn detail_returns_contract_fields() {
    let (_directory, adapter, store) = fixture();
    store
        .register_workspace(&workspace("workspace-a"))
        .expect("workspace");
    let mut target = goal(
        "goal-detail",
        ScopeRef::workspace("workspace-a").expect("scope"),
        100,
    );
    target.acceptance = vec!["验收一".to_owned(), "验收二".to_owned()];
    store.create_goal(&target).expect("goal");

    let (status, body) = get(&adapter, &detail_path("goal-detail"), None);
    assert_eq!(status, 200);
    assert_eq!(body["goal"]["goalId"], "goal-detail");
    assert_eq!(body["objective"], "完成 goal-detail");
    assert_eq!(body["steps"].as_array().map(Vec::len), Some(2));
    assert_eq!(body["acceptance"][0], "验收一");
    assert_eq!(body["objectiveTruncated"], false);
    assert_eq!(body["listsTruncated"], false);
    assert_eq!(body["recentExecutions"].as_array().map(Vec::len), Some(0));
    assert!(body["latestResult"].is_null());
    assert!(body["cancellationReason"].is_null());
}

#[test]
fn detail_reports_goal_not_found() {
    let (_directory, adapter, _store) = fixture();
    let (status, body) = get(&adapter, &detail_path("goal-missing"), None);
    assert_eq!(status, 404);
    assert_eq!(body["error"]["code"], "goal_not_found");
    assert_eq!(body["error"]["message"], "目标不存在或已被清理");
}

#[test]
fn unrelated_methods_and_paths_fall_through() {
    let (_directory, adapter, _store) = fixture();
    // 只读面不接受写方法，未命中即交回组合器，由网关给出统一 404。
    assert!(
        adapter
            .handle_request("POST", &list_path(), None, &[])
            .is_none()
    );
    assert!(
        adapter
            .handle_request("GET", &format!("{WORKBENCH_PATH_PREFIX}/lanes"), None, &[])
            .is_none()
    );
    assert!(
        adapter
            .handle_request("GET", &format!("{WORKBENCH_PATH_PREFIX}/goalsX"), None, &[])
            .is_none()
    );
    assert!(
        adapter
            .handle_request(
                "GET",
                &format!("{WORKBENCH_PATH_PREFIX}/goals/goal-1/nested"),
                None,
                &[]
            )
            .is_none()
    );
}
