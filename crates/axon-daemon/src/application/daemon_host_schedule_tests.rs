//! 调度记录的任务定义侧：省略 `--session-id` 时由定义侧落定目标会话。

use super::*;

use axon_domain::{ConsumerCursor, RuntimeHostId};
use axon_session::{
    CheckpointTelemetry, SessionEventRecord, SqliteRuntimeStore, WorkspaceRecord, WorkspaceStore,
};
use axon_session_runtime::{ActionExecutionRequest, TurnExecutionError, TurnExecutionOutcome};

const TEST_NOW_MS: u64 = 7_000;
const TEST_LEASE_DURATION_MS: u64 = 60_000;
const TEST_SCHEDULE_KIND: &str = "heartbeat";
const TEST_DELIVERY_MODE: &str = "follow_up";
const TEST_INTERVAL_MS: u64 = 60_000;

#[derive(Clone, Copy)]
struct NoopExecutor;

impl TurnExecutor for NoopExecutor {
    fn execute(
        &self,
        _request: ActionExecutionRequest,
    ) -> Result<TurnExecutionOutcome, TurnExecutionError> {
        Ok(TurnExecutionOutcome {
            events: Vec::new(),
            next_execution: None,
            checkpoint: CheckpointTelemetry::default(),
        })
    }
}

#[derive(Clone, Copy)]
struct NoopPublisher;

impl EventPublisher for NoopPublisher {
    fn publish(&self, _event: &SessionEventRecord, _cursor: &ConsumerCursor) -> Result<(), String> {
        Ok(())
    }
}

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

fn test_host(
    store: SqliteRuntimeStore,
) -> DaemonHost<SqliteRuntimeStore, NoopExecutor, NoopPublisher, FixedClock, TestProcess> {
    DaemonHost::new(
        store,
        NoopExecutor,
        NoopPublisher,
        FixedClock,
        TestProcess,
        RuntimeHostId::new("daemon-schedule-host").expect("Host ID"),
        TEST_LEASE_DURATION_MS,
        TEST_LEASE_DURATION_MS,
    )
}

/// 登记一个 Workspace：新建的定时任务会话从它继承工作区与根目录。
fn register_workspace(store: &SqliteRuntimeStore) {
    store
        .register_workspace(&WorkspaceRecord {
            workspace_id: "workspace-a".to_owned(),
            display_name: "workspace-a".to_owned(),
            workspace_root: "E:/work".to_owned(),
            git_remote: None,
            default_branch: None,
            aliases: Vec::new(),
            memory_namespace: "workspace-a".to_owned(),
            pinned: false,
            last_seen_at_ms: TEST_NOW_MS,
        })
        .expect("登记 Workspace");
}

fn payload() -> serde_json::Value {
    serde_json::json!({"prompt": "检查进度", "workspaceId": "workspace-a"})
}

#[test]
fn schedule_without_session_binds_a_cron_session_at_definition() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    register_workspace(&store);
    let mut host = test_host(store.clone());
    host.start().expect("启动 Host");

    let first = host
        .upsert_interval_schedule(
            "heartbeat-a".to_owned(),
            None,
            TEST_SCHEDULE_KIND.to_owned(),
            TEST_INTERVAL_MS,
            TEST_DELIVERY_MODE.to_owned(),
            payload(),
        )
        .expect("创建调度");
    let session_id = first.session_id.clone().expect("定义侧必须落定目标会话");
    assert!(
        axon_session::is_ulid(session_id.as_str()),
        "定时任务会话标识必须是 ULID：{}",
        session_id.as_str()
    );
    let record = store.get_session(&session_id).expect("会话应已登记");
    assert_eq!(record.kind, axon_session::SessionKind::Cron);
    assert_eq!(record.workspace_id.as_deref(), Some("workspace-a"));
    assert_eq!(record.cwd, "E:/work", "会话工作目录取自 Workspace 根目录");

    // 同一个任务再次定义（仍省略会话）必须沿用同一条会话。
    let second = host
        .upsert_interval_schedule(
            "heartbeat-a".to_owned(),
            None,
            TEST_SCHEDULE_KIND.to_owned(),
            TEST_INTERVAL_MS,
            TEST_DELIVERY_MODE.to_owned(),
            payload(),
        )
        .expect("再次定义");
    assert_eq!(second.session_id.as_ref(), Some(&session_id));
}

#[test]
fn explicit_session_is_kept_and_validated() {
    let directory = axon_test_support::test_temp_dir();
    let store = SqliteRuntimeStore::new(directory.path().join("runtime.sqlite3"));
    register_workspace(&store);
    let mut host = test_host(store.clone());
    host.start().expect("启动 Host");
    let explicit = DaemonSessionId::new("01J8Z0000000000000000000S1").expect("会话标识");
    store
        .create_session(&explicit, "E:/work", TEST_NOW_MS)
        .expect("登记会话");

    let schedule = host
        .upsert_interval_schedule(
            "heartbeat-b".to_owned(),
            Some(explicit.clone()),
            TEST_SCHEDULE_KIND.to_owned(),
            TEST_INTERVAL_MS,
            TEST_DELIVERY_MODE.to_owned(),
            payload(),
        )
        .expect("显式会话应被沿用");
    assert_eq!(schedule.session_id, Some(explicit.clone()));

    // 未登记的会话在定义侧就被拒：不能留下一条唤醒时才报错的调度。
    let stranger = DaemonSessionId::new("01J8Z0000000000000000000S9").expect("会话标识");
    assert!(
        host.upsert_interval_schedule(
            "heartbeat-c".to_owned(),
            Some(stranger),
            TEST_SCHEDULE_KIND.to_owned(),
            TEST_INTERVAL_MS,
            TEST_DELIVERY_MODE.to_owned(),
            payload(),
        )
        .is_err()
    );
}
