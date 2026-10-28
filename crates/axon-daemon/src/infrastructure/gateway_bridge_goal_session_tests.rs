//! 目标执行会话的订阅放行契约测试：执行绑定与工作区双重校验，且子代理路径不变。

use super::*;
use crate::application::{EventStreamBridge, NoActiveTurnSteerSink};
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, GoalPlan, GoalRecord, GoalStatus, ScopeRef,
};
use axon_session::{
    ExecutionStore, GoalStore, RuntimeStore, SqliteRuntimeStore, WorkspaceRecord, WorkspaceStore,
};

const ACTION_LEASE_DURATION_MS: u64 = 120_000;
const NOW_MS: u64 = 1_760_000_000_000;
const OTHER_WORKSPACE: &str = "workspace-other";

/// 固定时钟：判定与 lease 都不依赖真实时间。
#[derive(Clone)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        NOW_MS
    }
}

struct Fixture {
    store: SqliteRuntimeStore,
    stream_bridge: Arc<EventStreamBridge>,
    // 字段顺序即析构顺序：临时目录最后析构，先释放 SQLite 句柄。
    _temp: axon_test_support::TestTempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = axon_test_support::test_temp_dir();
        let data_root = temp.path().join("data");
        let store = SqliteRuntimeStore::from_root(&data_root);
        store.initialize().expect("初始化运行态");
        Self {
            store,
            stream_bridge: Arc::new(EventStreamBridge::new()),
            _temp: temp,
        }
    }

    fn bridge(&self) -> DaemonGatewayBridge<SqliteRuntimeStore, FixedClock> {
        DaemonGatewayBridge::new(GatewayHostDependencies {
            store: self.store.clone(),
            active_turn_steer_sink: Arc::new(NoActiveTurnSteerSink),
            clock: FixedClock,
            action_lease_duration_ms: ACTION_LEASE_DURATION_MS,
            stream_bridge: Arc::clone(&self.stream_bridge),
            host_generation: None,
        })
    }

    fn register_workspace(&self, workspace_id: &str, root: &str) {
        self.store
            .register_workspace(&WorkspaceRecord {
                workspace_id: workspace_id.to_owned(),
                display_name: workspace_id.to_owned(),
                workspace_root: root.to_owned(),
                git_remote: None,
                default_branch: None,
                aliases: Vec::new(),
                memory_namespace: workspace_id.to_owned(),
                pinned: false,
                last_seen_at_ms: 1,
            })
            .expect("注册工作区");
    }

    /// 建目标、建会话、建执行绑定，返回该执行绑定的会话标识。
    fn goal_with_execution(&self, goal_id: &str, scope: ScopeRef, cwd: &str) -> DaemonSessionId {
        let mut goal = GoalRecord::new(
            axon_domain::GoalId::new(goal_id).expect("goal id"),
            scope,
            "订阅放行",
            GoalPlan {
                objective: "校验目标执行会话订阅".to_owned(),
                steps: vec!["建立绑定".to_owned()],
            },
            vec!["放行或拒绝正确".to_owned()],
            NOW_MS,
        )
        .expect("goal");
        goal.transition(GoalStatus::Active, NOW_MS + 1)
            .expect("active");
        self.store.create_goal(&goal).expect("落库目标");

        let session_id =
            DaemonSessionId::new(format!("goal-session-{goal_id}")).expect("session id");
        self.store
            .create_session(&session_id, cwd, NOW_MS)
            .expect("登记会话");
        let mut execution = ExecutionRecord::new(
            ExecutionId::new(format!("execution-{goal_id}")).expect("execution id"),
            goal.goal_id.clone(),
            NOW_MS,
        );
        execution.session_id = Some(session_id.clone());
        self.store.enqueue_execution(&execution).expect("落库执行");
        session_id
    }

    fn subscribe(
        &self,
        workspace_id: &str,
        session_id: &DaemonSessionId,
    ) -> Result<(), GatewayKernelError> {
        let bridge = self.bridge();
        GatewayEgress::subscribe(
            &bridge,
            GatewaySubscriptionRequest {
                workspace_id: workspace_id.to_owned(),
                interaction_id: String::new(),
                session_id: Some(session_id.as_str().to_owned()),
                resume_cursor: None,
            },
        )
        .map(|_| ())
    }
}

#[test]
fn goal_session_with_matching_workspace_is_allowed() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    let session_id = fixture.goal_with_execution(
        "goal-ws",
        ScopeRef::workspace("workspace-a").expect("scope"),
        "/repo-a",
    );
    assert!(fixture.subscribe("workspace-a", &session_id).is_ok());
}

#[test]
fn goal_session_bound_to_another_workspace_is_rejected() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    let session_id = fixture.goal_with_execution(
        "goal-ws",
        ScopeRef::workspace("workspace-a").expect("scope"),
        "/repo-a",
    );
    // 会话与执行绑定都在，但请求工作区不符：必须拒绝，不能因为「有绑定」就放行任意工作区。
    let error = fixture
        .subscribe(OTHER_WORKSPACE, &session_id)
        .expect_err("工作区不符应拒绝");
    assert!(
        matches!(error, GatewayKernelError::Ingress(ref reason) if reason.contains("工作区不匹配")),
        "{error:?}"
    );
}

#[test]
fn session_without_execution_binding_is_rejected() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    // 普通会话：既不是子代理会话，也没有执行绑定。
    let session_id = DaemonSessionId::new("lane-session-plain").expect("session id");
    fixture
        .store
        .create_session(&session_id, "/repo-a", NOW_MS)
        .expect("登记会话");
    let error = fixture
        .subscribe("workspace-a", &session_id)
        .expect_err("无执行绑定应拒绝");
    assert!(
        matches!(error, GatewayKernelError::Ingress(ref reason) if reason.contains("目标执行绑定")),
        "{error:?}"
    );
}

#[test]
fn subagent_session_keeps_previous_behaviour() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    // 子代理会话没有执行绑定，也不做工作区比对：这条分支的行为必须与改造前逐字一致。
    let session_id = DaemonSessionId::new("run-uuid--subagent--root").expect("subagent session id");
    fixture
        .store
        .create_session(&session_id, "/elsewhere", NOW_MS)
        .expect("登记会话");
    assert!(fixture.subscribe("workspace-a", &session_id).is_ok());
}

#[test]
fn subagent_session_without_parent_prefix_is_rejected() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    let session_id = DaemonSessionId::new("--subagent--root").expect("session id");
    fixture
        .store
        .create_session(&session_id, "/repo-a", NOW_MS)
        .expect("登记会话");
    let error = fixture
        .subscribe("workspace-a", &session_id)
        .expect_err("缺少父会话前缀应拒绝");
    assert!(
        matches!(error, GatewayKernelError::Ingress(ref reason) if reason.contains("父会话前缀")),
        "{error:?}"
    );
}

#[test]
fn global_goal_resolves_workspace_from_session_cwd() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    // 全局目标的 cwd 精确命中的唯一工作区即它的工作区。
    let session_id = fixture.goal_with_execution("goal-global", ScopeRef::global(), "/repo-a");
    assert!(fixture.subscribe("workspace-a", &session_id).is_ok());
    assert!(fixture.subscribe(OTHER_WORKSPACE, &session_id).is_err());
}

#[test]
fn global_goal_with_unresolvable_cwd_is_rejected() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    let session_id = fixture.goal_with_execution("goal-global", ScopeRef::global(), "/nowhere");
    // 零个匹配一律拒绝，不做「猜一个最像的」。
    assert!(fixture.subscribe("workspace-a", &session_id).is_err());
}

#[test]
fn global_goal_with_ambiguous_cwd_is_rejected() {
    let fixture = Fixture::new();
    // 两个工作区登记同一根目录：匹配不唯一，因此拒绝。
    fixture.register_workspace("workspace-a", "/repo-a");
    fixture.register_workspace("workspace-b", "/repo-a");
    let session_id = fixture.goal_with_execution("goal-global", ScopeRef::global(), "/repo-a");
    assert!(fixture.subscribe("workspace-a", &session_id).is_err());
}

#[test]
fn execution_bound_to_missing_goal_is_rejected() {
    let fixture = Fixture::new();
    fixture.register_workspace("workspace-a", "/repo-a");
    let session_id = DaemonSessionId::new("goal-session-orphan").expect("session id");
    fixture
        .store
        .create_session(&session_id, "/repo-a", NOW_MS)
        .expect("登记会话");
    let execution = {
        let mut record = ExecutionRecord::new(
            ExecutionId::new("execution-orphan").expect("execution id"),
            axon_domain::GoalId::new("goal-deleted").expect("goal id"),
            NOW_MS,
        );
        record.session_id = Some(session_id.clone());
        record
    };
    // 目标已不存在时执行绑定仍在：不得因为「查得到执行」就放行。
    fixture
        .store
        .enqueue_execution(&execution)
        .expect("落库执行");
    assert!(fixture.subscribe("workspace-a", &session_id).is_err());
}
