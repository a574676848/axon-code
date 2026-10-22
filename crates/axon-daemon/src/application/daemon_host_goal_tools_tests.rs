//! Daemon 主 turn Goal 工具命令映射测试。
//!
//! 覆盖工具写入路径的关键事实：创建事务的幂等性、首个 Execution 的
//! Worker 门禁授权、CAS 冲突、取消持久化与跨 Workspace 可见性隔离。

use super::*;
use crate::ports::ProcessIdentity;
use axon_domain::{
    ConsumerCursor, DisclosurePolicy, ExecutionId, ExecutionStatus, GoalListQuery, GoalQueryScope,
    GoalStatus, GoalUpdateCommand, RuntimeHostId,
};
use axon_session::{
    CheckpointTelemetry, DecisionStore, ExecutionStore, GoalStore, SessionEventRecord,
    SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{
    ActionExecutionRequest, RuntimeClock, TurnExecutionError, TurnExecutionOutcome, TurnExecutor,
    model::RuntimeEvent,
};
use std::path::Path;

const TEST_NOW_MS: u64 = 10;
const TEST_LEASE_DURATION_MS: u64 = 1_000;

#[derive(Clone, Copy)]
struct FixedClock;

impl RuntimeClock for FixedClock {
    fn now_ms(&self) -> u64 {
        TEST_NOW_MS
    }
}

#[derive(Clone, Copy)]
struct TestProcess;

impl ProcessIdentity for TestProcess {
    fn pid(&self) -> u32 {
        1
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Default)]
struct AnswerExecutor;

impl TurnExecutor for AnswerExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: "turn_completed".to_owned(),
                payload_json: json!({"response": "测试回复"}).to_string(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

type TestHost =
    DaemonHost<SqliteRuntimeStore, AnswerExecutor, NoopPublisher, FixedClock, TestProcess>;

fn start_host(store: &SqliteRuntimeStore, host_id: &str) -> TestHost {
    let mut host = DaemonHost::new(
        store.clone(),
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new(host_id).expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    );
    host.start().expect("启动 Host");
    host
}

fn register_workspace(store: &SqliteRuntimeStore, path: &Path) -> String {
    let workspace = WorkspaceResolver
        .resolve(path, TEST_NOW_MS)
        .expect("解析 Workspace");
    store
        .register_workspace(&workspace)
        .expect("注册 Workspace");
    workspace.workspace_id.as_str().to_owned()
}

fn create_command(create_execution: bool) -> GoalCreateCommand {
    GoalCreateCommand {
        scope: GoalCreationScope::CurrentWorkspace,
        title: "整理发布说明".to_owned(),
        objective: "输出可验收的发布说明".to_owned(),
        steps: vec!["汇总变更".to_owned()],
        acceptance: vec!["发布说明通过审阅".to_owned()],
        create_execution,
    }
}

fn tool_request(
    workspace_id: Option<&str>,
    idempotency_key: &str,
    command: GoalToolCommand,
) -> GoalToolRequest {
    GoalToolRequest {
        actor: "tester".to_owned(),
        workspace_id: workspace_id.map(str::to_owned),
        idempotency_key: idempotency_key.to_owned(),
        source: Some(axon_protocol::GoalSource::OneOff),
        disclosure_policy: Some(DisclosurePolicy::PrivateOnly),
        transport_context: Some(
            serde_json::json!({"channel":"tui","destination":"tester","audience": axon_domain::AudienceContext::new(axon_domain::AudienceKind::Private, DisclosurePolicy::PrivateOnly)}),
        ),
        command,
    }
}

fn goal_id_of(value: &Value) -> GoalId {
    GoalId::new(value["goalId"].as_str().expect("回执包含 goalId")).expect("Goal ID 有效")
}

#[test]
fn create_with_execution_records_scope_allow_decision() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-create-host");

    let receipt = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "create-with-execution",
            GoalToolCommand::Create(create_command(true)),
        ))
        .expect("创建 Goal 成功");
    // 新建 Goal 处于 proposed 初始态：只有 Worker 实际推进后才进入 active。
    assert_eq!(receipt["status"], json!(GoalStatus::Proposed));

    let goal_id = goal_id_of(&receipt);
    let goal = store
        .get_goal(&goal_id)
        .expect("读取 Goal")
        .expect("Goal 必须已落库");
    let execution_id = ExecutionId::new(format!("execution-{goal_id}")).expect("Execution ID 有效");
    let execution = store
        .get_execution(&execution_id)
        .expect("读取 Execution")
        .expect("首个 Execution 必须已落库");
    assert_eq!(execution.status, ExecutionStatus::Queued);
    assert_eq!(execution.goal_id, goal.goal_id);
    // Worker 门禁按作用域最新 Decision 判定；缺失时首个 Execution 会被直接取消。
    let decision = store
        .get_latest_decision(&goal.scope, TEST_NOW_MS)
        .expect("读取 Decision")
        .expect("Goal 创建必须写入授权 Decision");
    assert!(decision.is_effective_at(TEST_NOW_MS));
}

#[test]
fn cron_source_persists_explicit_delivery_target_in_execution_payload() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "cron-target-host");
    let mut request = tool_request(
        Some(&workspace_id),
        "cron-target",
        GoalToolCommand::Create(create_command(true)),
    );
    request.source = Some(axon_protocol::GoalSource::AxonCron {
        job_id: "job-42".to_owned(),
        channel: "feishu".to_owned(),
        destination: "oc_test".to_owned(),
    });

    let receipt = host.goal_tool_value(request).expect("创建 cron Goal 成功");
    let goal_id = goal_id_of(&receipt);
    let execution_id = ExecutionId::new(format!("execution-{goal_id}")).expect("Execution ID");
    let execution = store
        .get_execution(&execution_id)
        .expect("读取 Execution")
        .expect("Execution 必须存在");
    let payload: Value = serde_json::from_str(&execution.payload_json).expect("payload JSON");
    assert_eq!(payload["transportContext"]["channel"], "feishu");
    assert_eq!(payload["transportContext"]["destination"], "oc_test");
    assert_eq!(payload["transportContext"]["source"]["kind"], "axon_cron");
}

#[test]
fn create_reuses_goal_for_same_idempotency_key() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-idempotent-host");

    let first = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "repeat-key",
            GoalToolCommand::Create(create_command(false)),
        ))
        .expect("首次创建成功");
    let second = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "repeat-key",
            GoalToolCommand::Create(create_command(false)),
        ))
        .expect("重复创建返回既有 Goal");

    assert_eq!(first["created"], json!(true));
    assert_eq!(second["created"], json!(false));
    assert_eq!(first["goalId"], second["goalId"]);
    assert!(second["tool"] == json!("goal_create"));
}

#[test]
fn update_conflicts_on_stale_revision() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-update-host");

    let receipt = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "update-cas",
            GoalToolCommand::Create(create_command(false)),
        ))
        .expect("创建 Goal 成功");
    let goal_id = goal_id_of(&receipt);
    let revision = receipt["revision"].as_u64().expect("回执包含 revision");

    let stale = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "update-cas",
            GoalToolCommand::Update(GoalUpdateCommand {
                goal_id: goal_id.clone(),
                expected_revision: revision + 1,
                title: Some("过期写入".to_owned()),
                waiting_reason: None,
                next_step: None,
                priority: None,
                status: None,
            }),
        ))
        .expect_err("过期 revision 必须被拒绝");
    assert!(stale.to_string().contains("revision"));

    let updated = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "update-cas",
            GoalToolCommand::Update(GoalUpdateCommand {
                goal_id: goal_id.clone(),
                expected_revision: revision,
                title: Some("新标题".to_owned()),
                waiting_reason: None,
                next_step: Some("继续汇总".to_owned()),
                priority: None,
                status: None,
            }),
        ))
        .expect("合法 revision 更新成功");
    assert_eq!(updated["revision"], json!(revision + 1));
}

#[test]
fn cancel_persists_reason_and_terminal_status() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-cancel-host");

    let receipt = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "cancel-goal",
            GoalToolCommand::Create(create_command(false)),
        ))
        .expect("创建 Goal 成功");
    let goal_id = goal_id_of(&receipt);
    let revision = receipt["revision"].as_u64().expect("回执包含 revision");

    let cancelled = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "cancel-goal",
            GoalToolCommand::Cancel(GoalCancelCommand {
                goal_id: goal_id.clone(),
                expected_revision: revision,
                reason: "用户取消".to_owned(),
            }),
        ))
        .expect("取消成功");
    assert_eq!(cancelled["status"], json!(GoalStatus::Cancelled));

    let goal = store
        .get_goal(&goal_id)
        .expect("读取 Goal")
        .expect("Goal 必须已落库");
    assert_eq!(goal.status, GoalStatus::Cancelled);
    assert_eq!(goal.cancellation_reason.as_deref(), Some("用户取消"));
}

#[test]
fn cancellation_fact_survives_independent_store_handle() {
    let directory = axon_test_support::test_temp_dir();
    let database_path = directory.path().join("runtime.sqlite3");
    let store = SqliteRuntimeStore::new(database_path.clone());
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-cancel-reopen-host");
    let receipt = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "cancel-reopen-goal",
            GoalToolCommand::Create(create_command(false)),
        ))
        .expect("创建 Goal 成功");
    let goal_id = goal_id_of(&receipt);
    let revision = receipt["revision"].as_u64().expect("回执包含 revision");
    host.goal_tool_value(tool_request(
        Some(&workspace_id),
        "cancel-reopen-goal",
        GoalToolCommand::Cancel(GoalCancelCommand {
            goal_id: goal_id.clone(),
            expected_revision: revision,
            reason: "重启前取消".to_owned(),
        }),
    ))
    .expect("取消成功");

    let reopened = SqliteRuntimeStore::new(database_path);
    let goal = reopened
        .get_goal(&goal_id)
        .expect("重启后读取 Goal")
        .expect("Goal 必须存在");
    assert_eq!(goal.status, GoalStatus::Cancelled);
    assert_eq!(goal.cancellation_reason.as_deref(), Some("重启前取消"));
}

#[test]
fn get_hides_goal_outside_bound_workspace() {
    let directory = axon_test_support::test_temp_dir();
    let other_directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let other_workspace_id = register_workspace(&store, other_directory.path());
    let host = start_host(&store, "goal-scope-host");

    let receipt = host
        .goal_tool_value(tool_request(
            Some(&workspace_id),
            "scoped-goal",
            GoalToolCommand::Create(create_command(false)),
        ))
        .expect("创建 Goal 成功");
    let goal_id = goal_id_of(&receipt);

    let error = host
        .goal_tool_value(tool_request(
            Some(&other_workspace_id),
            "scoped-goal",
            GoalToolCommand::Get(GoalDetailQuery {
                goal_id,
                include_executions: false,
                include_events: false,
                limit: 1,
            }),
        ))
        .expect_err("跨 Workspace 读取必须被拒绝");
    assert!(error.to_string().contains("不可见"));
}

#[test]
fn unregistered_workspace_keeps_global_goal_available() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = start_host(&store, "goal-unregistered-workspace-host");
    let unregistered_workspace_id = "workspace-not-registered";

    let receipt = host
        .goal_tool_value(tool_request(
            Some(unregistered_workspace_id),
            "unregistered-global-goal",
            GoalToolCommand::Create(GoalCreateCommand {
                scope: GoalCreationScope::Global,
                title: "整理全局承诺".to_owned(),
                objective: "验证未注册 Workspace 下全局 Goal 仍可创建".to_owned(),
                steps: vec!["创建全局 Goal".to_owned()],
                acceptance: vec!["Goal 落库可供全局查询".to_owned()],
                create_execution: false,
            }),
        ))
        .expect("未注册 Workspace 下仍可创建全局 Goal");
    goal_id_of(&receipt);

    let listed = host
        .goal_tool_value(tool_request(
            Some(unregistered_workspace_id),
            "unregistered-global-list",
            GoalToolCommand::List(GoalListQuery {
                scope: GoalQueryScope::Global,
                statuses: Vec::new(),
                limit: 20,
                cursor: None,
            }),
        ))
        .expect("未注册 Workspace 下仍可查询全局 Goal");
    let goals = listed["goals"].as_array().expect("列表返回数组");
    assert_eq!(goals.len(), 1);
}

#[test]
fn unregistered_workspace_rejects_current_workspace_scope() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let host = start_host(&store, "goal-unregistered-scope-host");

    let error = host
        .goal_tool_value(tool_request(
            Some("workspace-not-registered"),
            "unregistered-scoped-list",
            GoalToolCommand::List(GoalListQuery {
                scope: GoalQueryScope::CurrentWorkspace,
                statuses: Vec::new(),
                limit: 20,
                cursor: None,
            }),
        ))
        .expect_err("未注册 Workspace 下 current_workspace 查询必须被拒绝");
    assert!(error.to_string().contains("需要绑定 Workspace"));
}

#[test]
fn workspace_disclosure_shares_workspace_ledger_but_hides_global_goals() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-disclosure-host");

    let private_request = |key: &str, command: GoalToolCommand| {
        let mut request = tool_request(Some(&workspace_id), key, command);
        request.disclosure_policy = Some(DisclosurePolicy::PrivateOnly);
        request
    };
    let group_request = |key: &str, command: GoalToolCommand| {
        let mut request = tool_request(Some(&workspace_id), key, command);
        request.disclosure_policy = Some(DisclosurePolicy::Workspace);
        request
    };

    let workspace_goal = goal_id_of(
        &host
            .goal_tool_value(private_request(
                "shared-workspace-goal",
                GoalToolCommand::Create(create_command(false)),
            ))
            .expect("私聊创建 Workspace Goal 成功"),
    );
    let global_goal = goal_id_of(
        &host
            .goal_tool_value(private_request(
                "private-global-goal",
                GoalToolCommand::Create(GoalCreateCommand {
                    scope: GoalCreationScope::Global,
                    ..create_command(false)
                }),
            ))
            .expect("私聊创建 Global Goal 成功"),
    );
    let global_goal_id = global_goal.clone();

    let shared_list = group_request(
        "group-shared-list",
        GoalToolCommand::List(GoalListQuery {
            scope: GoalQueryScope::CurrentWorkspace,
            statuses: Vec::new(),
            limit: 20,
            cursor: None,
        }),
    );
    let listed = host
        .goal_tool_value(shared_list)
        .expect("群聊可读取同 Workspace Work Ledger");
    let goals = listed["goals"].as_array().expect("列表返回数组");
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0]["goal_id"], json!(workspace_goal.as_str()));

    let global_error = host
        .goal_tool_value(group_request(
            "group-global-list",
            GoalToolCommand::List(GoalListQuery {
                scope: GoalQueryScope::Global,
                statuses: Vec::new(),
                limit: 20,
                cursor: None,
            }),
        ))
        .expect_err("群聊不能查询 Global Goal");
    assert!(global_error.to_string().contains("Workspace Goal"));

    let get_error = host
        .goal_tool_value(group_request(
            "group-global-get",
            GoalToolCommand::Get(GoalDetailQuery {
                goal_id: global_goal_id.clone(),
                include_executions: false,
                include_events: false,
                limit: 1,
            }),
        ))
        .expect_err("群聊不能读取 Global Goal 详情");
    assert!(get_error.to_string().contains("不可见"));

    let update_error = host
        .goal_tool_value(group_request(
            "group-global-update",
            GoalToolCommand::Update(GoalUpdateCommand {
                goal_id: global_goal_id,
                expected_revision: 0,
                title: Some("越权更新".to_owned()),
                waiting_reason: None,
                next_step: None,
                priority: None,
                status: None,
            }),
        ))
        .expect_err("群聊不能更新 Global Goal");
    assert!(update_error.to_string().contains("不可见"));
}
