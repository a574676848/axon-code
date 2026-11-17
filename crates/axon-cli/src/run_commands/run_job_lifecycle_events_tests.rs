//! 作业生命周期落盘测试：验收「事件真的进了会话事件流」「指标投影有生产者」
//! 与「不重复登记会话行」。
//!
//! 每条用例使用自己的会话 ID：启动计数是进程级静态，共用 ID 会互相污染。

use std::path::PathBuf;

use axon_domain::DaemonSessionId;
use axon_session::{
    EventLogStore as _, JsonlThreadEventStore, RuntimeStore, SqliteRuntimeStore, ThreadService,
    ThreadSettings, ThreadSource,
};

use super::{LifecycleRecord, persist, take_started_count};

struct Fixture {
    _directory: axon_test_support::TestTempDir,
    config_path: PathBuf,
    store: SqliteRuntimeStore,
    session_id: String,
}

fn fixture(session: &str) -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态库");
    let session_id = DaemonSessionId::new(session.to_owned()).expect("合法会话 ID");
    store
        .create_session(&session_id, "/workspace", 1)
        .expect("登记会话");
    Fixture {
        _directory: directory,
        config_path,
        store,
        session_id: session.to_owned(),
    }
}

fn session_id(fixture: &Fixture) -> DaemonSessionId {
    DaemonSessionId::new(fixture.session_id.clone()).expect("合法会话 ID")
}

fn read_job_events(
    store: &SqliteRuntimeStore,
    session_id: &DaemonSessionId,
) -> Vec<(String, String)> {
    store
        .replay_events(session_id, 0, 32)
        .expect("读取事件")
        .into_iter()
        .map(|record| (record.event_type, record.payload_json))
        .filter(|(kind, _)| kind.starts_with("job_"))
        .collect()
}

fn jobs_running(store: &SqliteRuntimeStore, session_id: &DaemonSessionId) -> i64 {
    store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE((SELECT jobs_running FROM session_metrics WHERE session_id = ?1), 0)",
                [session_id.as_str()],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("读取作业计数")
}

fn started_record(session: &str) -> LifecycleRecord {
    LifecycleRecord {
        session_id: session.to_owned(),
        kind: "job_started",
        payload: r#"{"jobId":"1","kind":"shell","owner":"s","startedAtMs":10}"#.to_owned(),
        started: true,
        failed: false,
    }
}

fn completed_record(session: &str, status: &str) -> LifecycleRecord {
    LifecycleRecord {
        session_id: session.to_owned(),
        kind: "job_completed",
        payload: format!(r#"{{"jobId":"1","status":"{status}"}}"#),
        started: false,
        failed: false,
    }
}

#[test]
fn job_started_event_lands_in_the_session_event_log() {
    let fixture = fixture("run-jobs-log");
    persist(&fixture.config_path, started_record(&fixture.session_id));

    let events = read_job_events(&fixture.store, &session_id(&fixture));
    assert_eq!(events.len(), 1, "应恰好写入一条作业事件");
    assert_eq!(events[0].0, "job_started");
    assert!(events[0].1.contains("\"jobId\":\"1\""), "{}", events[0].1);
}

#[test]
fn job_events_drive_the_running_counter() {
    let fixture = fixture("run-jobs-counter");
    persist(&fixture.config_path, started_record(&fixture.session_id));
    assert_eq!(jobs_running(&fixture.store, &session_id(&fixture)), 1);

    persist(
        &fixture.config_path,
        completed_record(&fixture.session_id, "completed"),
    );
    assert_eq!(jobs_running(&fixture.store, &session_id(&fixture)), 0);
}

#[test]
fn start_count_is_per_session_and_drained_on_settlement() {
    let fixture = fixture("run-jobs-drain");
    assert_eq!(take_started_count(&fixture.session_id), 0);
    persist(&fixture.config_path, started_record(&fixture.session_id));
    assert_eq!(take_started_count(&fixture.session_id), 1);
    assert_eq!(
        take_started_count(&fixture.session_id),
        0,
        "取走后必须清零，否则下一轮会重复计数"
    );
}

#[test]
fn lane_owned_run_window_does_not_get_a_duplicate_session_row() {
    let fixture = fixture("run-jobs-lane");
    // 造一段「车道认领该运行窗口」的事实：车道会话的 run_session_id 就是这段窗口。
    let lane = DaemonSessionId::new("session-lane-owned".to_owned()).expect("合法车道 ID");
    let lane_row = fixture
        .store
        .create_session(&lane, "/workspace", 2)
        .expect("登记车道会话");
    let run_window = lane_row.run_session_id;

    persist(
        &fixture.config_path,
        completed_record(&run_window, "completed"),
    );

    let lane_owner_exists = fixture
        .store
        .with_read_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM sessions WHERE run_session_id = ?1",
                [run_window.as_str()],
                |row| row.get::<_, i64>(0),
            )
        })
        .expect("统计会话行");
    assert_eq!(lane_owner_exists, 1, "同一运行窗口只应有一条车道行");
    let rows = fixture
        .store
        .with_read_connection(|connection| {
            connection.query_row("SELECT COUNT(*) FROM sessions", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .expect("统计会话总数");
    assert_eq!(rows, 2, "不应为运行窗口补登重复会话行");
}

#[test]
fn job_events_survive_thread_materialization() {
    let fixture = fixture("run-jobs-thread");
    persist(
        &fixture.config_path,
        completed_record(&fixture.session_id, "killed"),
    );

    // 事件面与 thread 面互不影响：写事件不会顺带创建 thread 起始记录。
    let root = crate::runtime_paths::runtime_axon_root_path(&fixture.config_path).expect("运行根");
    ThreadService::new(JsonlThreadEventStore)
        .start_empty_thread(
            &root,
            axon_session::StartEmptyThreadCommand {
                thread_id: fixture.session_id.clone(),
                session_root_id: fixture.session_id.clone(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: "/workspace".to_owned(),
                    provider: "p-test".to_owned(),
                    model: "m-test".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 3,
            },
        )
        .expect("写入 thread");

    let events = read_job_events(&fixture.store, &session_id(&fixture));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, "job_completed");
}
