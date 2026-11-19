//! 目标执行走主 turn 管道的验收。
//!
//! 目标轮与用户轮共用一条管道，因此这里只验收三件事：目标启动会提交一条带目标归因的
//! Interaction、这条 Interaction 由主 turn 执行并回写账本、目标更新会把新的要求投进
//! 同一会话。

use super::*;
use crate::ports::ProcessIdentity;
use axon_domain::{
    ConsumerCursor, DisclosurePolicy, ExecutionId, ExecutionStatus, GoalCreateCommand,
    GoalCreationScope, GoalId, GoalStatus, RuntimeHostId,
};
use axon_protocol::{GoalToolCommand, GoalToolRequest};
use axon_session::{
    CheckpointTelemetry, EventLogStore, ExecutionStore, GoalStore, SessionEventRecord,
    SqliteRuntimeStore, WorkspaceResolver, WorkspaceStore,
};
use axon_session_runtime::{
    ActionExecutionRequest, RuntimeClock, TurnExecutionError, TurnExecutionOutcome, TurnExecutor,
    model::RuntimeEvent,
};
use serde_json::json;
use std::path::Path;

const TEST_NOW_MS: u64 = 10;
const TEST_LEASE_MS: u64 = 1_000;
/// 目标执行 Interaction 的来源前缀，与 Daemon 写入侧同源。
const GOAL_EXECUTION_PREFIX: &str = "goal-execution:";
/// 目标更新 Interaction 的来源前缀，与 Daemon 写入侧同源。
const GOAL_UPDATE_PREFIX: &str = "goal-update:";

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

#[derive(Clone, Copy, Default)]
struct AnswerExecutor;

impl TurnExecutor for AnswerExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: vec![RuntimeEvent {
                event_type: "turn_completed".to_owned(),
                payload_json: json!({"response": "目标已完成"}).to_string(),
            }],
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

type TestHost =
    DaemonHost<SqliteRuntimeStore, AnswerExecutor, NoopPublisher, FixedClock, TestProcess>;

#[derive(Debug)]
struct InboxRow {
    interaction_id: String,
    client: Option<String>,
    source_message_id: String,
    content: String,
}

fn start_host(store: &SqliteRuntimeStore, host_id: &str) -> TestHost {
    let mut host = DaemonHost::new(
        store.clone(),
        AnswerExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new(host_id).expect("Host ID"),
        TEST_LEASE_MS,
        TEST_LEASE_MS,
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

fn tool_request(
    workspace_id: &str,
    idempotency_key: &str,
    command: GoalToolCommand,
) -> GoalToolRequest {
    GoalToolRequest {
        actor: "tester".to_owned(),
        workspace_id: Some(workspace_id.to_owned()),
        idempotency_key: idempotency_key.to_owned(),
        source: Some(axon_protocol::GoalSource::OneOff),
        disclosure_policy: Some(DisclosurePolicy::PrivateOnly),
        transport_context: Some(serde_json::json!({
            "channel": "tui",
            "destination": "tester",
            "audience": {"kind": "private", "disclosure": "private_only"},
        })),
        command,
    }
}

fn create_command() -> GoalCreateCommand {
    GoalCreateCommand {
        scope: GoalCreationScope::CurrentWorkspace,
        title: "整理发布说明".to_owned(),
        objective: "输出可验收的发布说明".to_owned(),
        steps: vec!["汇总变更".to_owned()],
        acceptance: vec!["发布说明通过审阅".to_owned()],
        create_execution: true,
        write_policy: Default::default(),
    }
}

fn inbox_rows(store: &SqliteRuntimeStore) -> Vec<InboxRow> {
    store
        .with_read_connection(|connection| {
            connection
                .prepare(
                    "SELECT interaction_id, client, source_message_id, content FROM inbox \
                     ORDER BY received_at_ms ASC, interaction_id ASC",
                )
                .and_then(|mut statement| {
                    statement
                        .query_map([], |row| {
                            Ok(InboxRow {
                                interaction_id: row.get(0)?,
                                client: row.get(1)?,
                                source_message_id: row.get(2)?,
                                content: row.get(3)?,
                            })
                        })
                        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
                })
        })
        .expect("读取 inbox")
}

/// 创建一次带执行的目标，返回（目标标识、执行标识、目标会话）。
fn create_running_goal(
    host: &TestHost,
    store: &SqliteRuntimeStore,
    workspace_id: &str,
) -> (GoalId, ExecutionId, DaemonSessionId) {
    let receipt = host
        .goal_tool_value(tool_request(
            workspace_id,
            "goal-interaction-create",
            GoalToolCommand::Create(create_command()),
        ))
        .expect("创建目标");
    let goal_id = GoalId::new(receipt["goalId"].as_str().expect("回执含 goalId")).expect("Goal ID");
    let execution_id =
        ExecutionId::new(receipt["executionId"].as_str().expect("回执含 executionId"))
            .expect("Execution ID");
    let session_id = store
        .resolve_goal_session(&axon_session::GoalSessionRequest {
            goal_id: goal_id.as_str().to_owned(),
            workspace_id: Some(workspace_id.to_owned()),
            cwd: String::new(),
            now_ms: TEST_NOW_MS,
        })
        .expect("解析目标会话")
        .session_id;
    (goal_id, execution_id, session_id)
}

#[test]
fn goal_start_submits_a_goal_attributed_interaction() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-interaction-host");
    let (_, execution_id, _) = create_running_goal(&host, &store, &workspace_id);

    let rows = inbox_rows(&store);
    assert_eq!(rows.len(), 2, "工具轮加执行轮：{rows:?}");
    let dispatch = rows
        .iter()
        .find(|row| row.source_message_id.starts_with(GOAL_EXECUTION_PREFIX))
        .expect("目标执行轮必须落进 inbox");
    // 执行标识与首轮 interaction 同源：界面、账本与订阅用的是同一枚标识。
    assert_eq!(dispatch.interaction_id, execution_id.as_str());
    assert_eq!(dispatch.client.as_deref(), Some("goal"));
    assert!(
        dispatch.content.contains("整理发布说明"),
        "执行轮的正文是目标契约全文：{}",
        dispatch.content
    );
}

#[test]
fn goal_execution_runs_as_a_primary_turn_and_settles_the_ledger() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-execution-host");
    let (goal_id, execution_id, session_id) = create_running_goal(&host, &store, &workspace_id);

    let report = host
        .primary_turn_tick(DEFAULT_QUEUE_LIMIT)
        .expect("主 turn tick");
    assert_eq!(report.claimed, 1, "目标执行轮必须由主 turn 领取");
    assert_eq!(report.understood, 1, "目标执行轮必须正常收口");

    let execution = store
        .get_execution(&execution_id)
        .expect("读取执行账本")
        .expect("执行账本已存在");
    assert_eq!(execution.status, ExecutionStatus::Completed);
    let goal = store
        .get_goal(&goal_id)
        .expect("读取目标")
        .expect("目标已存在");
    assert_eq!(goal.status, GoalStatus::Completed);

    // 事件必须带本轮 interaction 归属，否则界面按 interaction 订阅时看不到正文。
    let events = store.replay_events(&session_id, 0, 64).expect("回放事件");
    assert!(
        events
            .iter()
            .any(|event| event.payload_json.contains(execution_id.as_str())),
        "目标执行的事件必须带 interaction 归属：{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| event.event_type == "turn_completed"),
        "目标执行必须落收口事件：{events:?}"
    );
}

#[test]
fn updating_a_running_goal_delivers_a_new_requirement() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    let workspace_id = register_workspace(&store, directory.path());
    let host = start_host(&store, "goal-update-host");
    let (goal_id, _, _) = create_running_goal(&host, &store, &workspace_id);

    let revision = store
        .get_goal(&goal_id)
        .expect("读取目标")
        .expect("目标已存在")
        .revision;
    let receipt = host
        .goal_tool_value(tool_request(
            &workspace_id,
            "goal-interaction-update",
            GoalToolCommand::Update(axon_domain::GoalUpdateCommand {
                goal_id: goal_id.clone(),
                expected_revision: revision,
                title: Some("整理发布说明（补充范围）".to_owned()),
                waiting_reason: None,
                next_step: Some("补上回滚说明".to_owned()),
                priority: None,
                status: None,
            }),
        ))
        .expect("更新目标");

    assert_eq!(receipt["dispatched"], json!(true), "更新必须投递新要求");
    let rows = inbox_rows(&store);
    assert!(
        rows.iter()
            .any(|row| row.source_message_id.starts_with(GOAL_UPDATE_PREFIX)),
        "目标会话必须收到更新后的新要求：{rows:?}"
    );
}
