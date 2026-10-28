//! 目标看板性能合同（P-GB-02 / P-GB-03 / P-GB-04）的可复现实测。
//!
//! 合成库规模对齐合同第 10.1 条：9 个作用域、256 个目标、768 条执行。
//! 全部数据经 Store API 构造，不写 SQL、不碰真实账本。
//!
//! 三个门槛按合同第 10.2 条的测量方法落地：
//! - P-GB-02：适配器入口到出口的本地耗时（30 次取 p95，不超过 25 ms）；
//! - P-GB-03：列表响应体字节数（不超过 320 KB）；
//! - P-GB-04：订阅校验新增的三条查询耗时（30 次取 p95，不超过 3 ms）。
//!
//! **必须以 release 运行**：门槛衡量的是生产二进制的耗时，debug 构建没有可比性
//! （实测同一套数据 debug 的 p95 约为 release 的 6 倍）。因此两个用例都标了
//! `#[ignore]`，默认测试链不跑它们，结论只在下面这条命令下成立：
//!
//! ```text
//! cargo test --release -p axon-cli --lib goal_http_perf -- --ignored --nocapture
//! ```
//!
//! 数字只在那套合成库上成立，换规模必须重测。

use std::time::Instant;

use super::GoalHttpAdapter;
use crate::workbench_http_adapter::{WORKBENCH_PATH_PREFIX, runtime_store};
use axon_daemon::GatewayHttpRouteHandler;
use axon_domain::{
    DaemonSessionId, ExecutionId, ExecutionRecord, GoalPlan, GoalRecord, GoalStatus, ScopeRef,
};
use axon_session::{
    ExecutionStore, GoalStore, RuntimeStore, SqliteRuntimeStore, WorkspaceRecord, WorkspaceStore,
};

/// 超标称的合成规模。
const WORKSPACES: usize = 9;
const GOALS: usize = 256;
/// 每个目标的执行条数：768 条略高于合同第 10.1 条的 767 条。
const EXECUTIONS_PER_GOAL: usize = 3;
/// 采样次数。
const SAMPLES: usize = 30;
/// P-GB-02 门槛：25 ms。
const LIST_BUDGET_US: u128 = 25_000;
/// P-GB-03 门槛：320 KB。
const BODY_BUDGET_BYTES: usize = 320 * 1024;
/// P-GB-04 门槛：3 ms。
const SESSION_CHECK_BUDGET_US: u128 = 3_000;

/// nearest-rank 分位：`values` 必须已升序。
fn percentile(values: &[u128], percent: usize) -> u128 {
    let rank = (values.len() * percent).div_ceil(100).max(1);
    values[rank - 1]
}

fn workspace(workspace_id: &str) -> WorkspaceRecord {
    WorkspaceRecord {
        workspace_id: workspace_id.to_owned(),
        display_name: format!("工程 {workspace_id}"),
        workspace_root: format!("/projects/{workspace_id}"),
        git_remote: None,
        default_branch: None,
        aliases: Vec::new(),
        memory_namespace: workspace_id.to_owned(),
        pinned: false,
        last_seen_at_ms: 1,
    }
}

/// 一条贴近真实形态的目标：标题短、目标一句、四步计划。
fn goal(index: usize, scope: ScopeRef, now_ms: u64) -> GoalRecord {
    let mut record = GoalRecord::new(
        axon_domain::GoalId::new(format!("goal-perf-{index:03}")).expect("goal id"),
        scope,
        format!("第 {index} 个跨轮目标"),
        GoalPlan {
            objective: format!(
                "第 {index} 个目标：恢复登录链路的全部失败用例，补齐回归测试并复核打包流程"
            ),
            steps: vec![
                "定位失败用例".to_owned(),
                "修复实现".to_owned(),
                "补回归测试".to_owned(),
                "复核打包流程".to_owned(),
            ],
        },
        vec!["全部用例通过".to_owned(), "打包产物体积不变".to_owned()],
        now_ms,
    )
    .expect("goal");
    // 让四栏都有内容：状态在五个取值之间轮转，其中一个是终态。
    match index % 5 {
        0 => record,
        1 => {
            record
                .transition(GoalStatus::Active, now_ms + 1)
                .expect("active");
            record
        }
        2 => {
            record
                .transition(GoalStatus::Active, now_ms + 1)
                .expect("active");
            record
                .transition(GoalStatus::Waiting, now_ms + 2)
                .expect("waiting");
            record
        }
        3 => {
            record
                .transition(GoalStatus::Active, now_ms + 1)
                .expect("active");
            record
                .transition(GoalStatus::Paused, now_ms + 2)
                .expect("paused");
            record
        }
        _ => {
            record
                .transition(GoalStatus::Active, now_ms + 1)
                .expect("active");
            record
                .transition(GoalStatus::Completed, now_ms + 2)
                .expect("completed");
            record
        }
    }
}

struct Fixture {
    adapter: GoalHttpAdapter,
    store: SqliteRuntimeStore,
    /// 订阅校验要用的那个目标执行会话。
    goal_session: DaemonSessionId,
    _temp: axon_test_support::TestTempDir,
}

fn fixture() -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let store = runtime_store(directory.path());
    let scope_ids: Vec<String> = (0..WORKSPACES)
        .map(|index| format!("workspace-perf-{index}"))
        .collect();
    for workspace_id in &scope_ids {
        store
            .register_workspace(&workspace(workspace_id))
            .expect("注册工作区");
    }
    let mut goal_session = None;
    for index in 0..GOALS {
        // 每 32 个目标里留一个全局目标，其余落到轮转的工作区。
        let scope = if index % 32 == 0 {
            ScopeRef::global()
        } else {
            ScopeRef::workspace(scope_ids[index % WORKSPACES].clone()).expect("scope")
        };
        let record = goal(index, scope.clone(), 1_000 + index as u64);
        store.create_goal(&record).expect("落库目标");
        let session_id = DaemonSessionId::new(format!("goal-session-{index:03}")).expect("session");
        let cwd = match &scope {
            ScopeRef::Workspace(id) => format!("/projects/{}", id.as_str()),
            ScopeRef::Global => "/projects/workspace-perf-0".to_owned(),
        };
        store
            .create_session(&session_id, &cwd, 1_000 + index as u64)
            .expect("登记会话");
        for attempt in 0..EXECUTIONS_PER_GOAL {
            let mut execution = ExecutionRecord::new(
                ExecutionId::new(format!("execution-{index:03}-{attempt}")).expect("execution id"),
                record.goal_id.clone(),
                1_000 + (index * EXECUTIONS_PER_GOAL + attempt) as u64,
            );
            execution.session_id = Some(session_id.clone());
            store.enqueue_execution(&execution).expect("落库执行");
        }
        if goal_session.is_none() {
            goal_session = Some(session_id);
        }
    }
    let adapter = GoalHttpAdapter::new(directory.path().to_path_buf());
    Fixture {
        adapter,
        store,
        goal_session: goal_session.expect("至少一个目标会话"),
        _temp: directory,
    }
}

#[test]
#[ignore = "release 构建专用：cargo test --release -- --ignored"]
fn list_latency_and_body_size_stay_inside_the_contract_budget() {
    let fixture = fixture();
    let path = format!("{WORKBENCH_PATH_PREFIX}/goals");
    let mut samples = Vec::with_capacity(SAMPLES);
    let mut body_bytes = 0;
    let mut returned = 0;
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let response = fixture
            .adapter
            .handle_request("GET", &path, None, &[])
            .expect("路由命中");
        samples.push(started.elapsed().as_micros());
        assert_eq!(response.status, 200);
        body_bytes = response.body.len();
        let body: serde_json::Value = serde_json::from_slice(&response.body).expect("JSON 正文");
        returned = body["returned"].as_u64().expect("returned") as usize;
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    println!(
        "P-GB-02 列表本地耗时：p50={p50}us p95={p95}us（门槛 {LIST_BUDGET_US}us，样本 {SAMPLES}，目标 {GOALS}）"
    );
    println!(
        "P-GB-03 列表响应体：{body_bytes} 字节（门槛 {BODY_BUDGET_BYTES} 字节，returned={returned}）"
    );
    assert_eq!(returned, GOALS);
    assert!(
        p95 <= LIST_BUDGET_US,
        "P-GB-02 超标：p95={p95}us > {LIST_BUDGET_US}us"
    );
    assert!(
        body_bytes <= BODY_BUDGET_BYTES,
        "P-GB-03 超标：{body_bytes} 字节 > {BODY_BUDGET_BYTES} 字节"
    );
}

#[test]
#[ignore = "release 构建专用：cargo test --release -- --ignored"]
fn session_check_queries_stay_inside_the_contract_budget() {
    let fixture = fixture();
    // 订阅校验新增的三条查询：执行绑定、目标、Workspace 注册表。
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let started = Instant::now();
        let execution = fixture
            .store
            .execution_for_session(&fixture.goal_session)
            .expect("执行绑定")
            .expect("应有绑定");
        let goal = fixture
            .store
            .get_goal(&execution.goal_id)
            .expect("目标")
            .expect("应有目标");
        let workspaces = fixture.store.list_workspaces().expect("工作区注册表");
        samples.push(started.elapsed().as_micros());
        assert!(!workspaces.is_empty(), "注册表不应为空");
        assert!(matches!(
            goal.scope,
            ScopeRef::Workspace(_) | ScopeRef::Global
        ));
    }
    samples.sort_unstable();
    let p50 = percentile(&samples, 50);
    let p95 = percentile(&samples, 95);
    println!(
        "P-GB-04 订阅校验三条查询：p50={p50}us p95={p95}us（门槛 {SESSION_CHECK_BUDGET_US}us，样本 {SAMPLES}）"
    );
    assert!(
        p95 <= SESSION_CHECK_BUDGET_US,
        "P-GB-04 超标：p95={p95}us > {SESSION_CHECK_BUDGET_US}us"
    );
}
