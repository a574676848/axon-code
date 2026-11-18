//! 会话 ID → Thread ID 解析测试。
//!
//! 关键口径：`sessions list` 给出的车道 ID 必须能解析到它落盘的 Thread；
//! `debug session list` 给出的 Thread ID 必须原样命中，并带回可写进恢复指针的会话身份。

use std::path::{Path, PathBuf};

use axon_domain::DaemonSessionId;
use axon_session::{
    JsonlThreadEventStore, RuntimeStore, SqliteRuntimeStore, StartEmptyThreadCommand,
    ThreadService, ThreadSettings, ThreadSource,
};
use rusqlite::params;

use super::{resolve_session_target, run_window_has_lane};

fn fixture() -> (axon_test_support::TestTempDir, PathBuf) {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    (directory, config_path)
}

fn store_for(config_path: &Path) -> SqliteRuntimeStore {
    let data_root = config_path.parent().expect("配置目录").join("data");
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().expect("初始化运行态库");
    store
}

/// 写入一条真实的 Thread 起始事件，让 Thread 在事件流里存在。
fn seed_thread(config_path: &Path, thread_id: &str) {
    let root = crate::runtime_paths::runtime_axon_root_path(config_path).expect("运行根");
    ThreadService::new(JsonlThreadEventStore)
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: thread_id.to_owned(),
                session_root_id: thread_id.to_owned(),
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
                created_at_ms: 1,
            },
        )
        .expect("写入 thread 起始事件");
}

fn session(session_id: &str) -> DaemonSessionId {
    DaemonSessionId::new(session_id.to_owned()).expect("合法会话 ID")
}

#[test]
fn lane_session_resolves_to_its_run_window() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let lane = session("session-lane-resolve");
    let record = store
        .create_session(&lane, "/workspace", 1)
        .expect("登记车道会话");
    seed_thread(&config_path, &record.current_run_id);

    let resolved = resolve_session_target(&config_path, lane.as_str()).expect("车道 ID 应可解析");

    assert_eq!(resolved.thread_id, record.current_run_id);
    assert_eq!(resolved.session_id, lane.as_str());
}

#[test]
fn thread_id_resolves_directly_and_keeps_own_session_identity() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let thread_id = "thread-resolve-direct";
    seed_thread(&config_path, thread_id);
    // 显式 `--session` 运行会以 Thread 身份登记一条会话：解析时必须优先用它。
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,
                         worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                         last_activity_at_ms,created_at_ms,updated_at_ms)
                     VALUES(?1,?2,NULL,'private','primary','/workspace','created',0,0,0,0,5,5,5)",
                    params![thread_id, "own-run-window"],
                )
                .map(|_| ())
        })
        .expect("写入会话行");

    let resolved = resolve_session_target(&config_path, thread_id).expect("Thread ID 应可解析");

    assert_eq!(resolved.thread_id, thread_id);
    assert_eq!(resolved.session_id, thread_id);
}

#[test]
fn thread_id_without_own_row_falls_back_to_owning_lane() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let lane = session("session-owning-lane");
    let record = store
        .create_session(&lane, "/workspace", 1)
        .expect("登记车道会话");
    seed_thread(&config_path, &record.current_run_id);

    let resolved =
        resolve_session_target(&config_path, &record.current_run_id).expect("Thread ID 应可解析");

    assert_eq!(resolved.thread_id, record.current_run_id);
    assert_eq!(resolved.session_id, lane.as_str());
}

#[test]
fn unknown_id_reports_not_found() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    store
        .create_session(&session("session-no-thread"), "/workspace", 1)
        .expect("登记会话");

    let error = resolve_session_target(&config_path, "session-no-thread")
        .expect_err("会话有登记但没有 Thread 时必须报错");

    assert!(
        error.contains("未找到会话"),
        "错误信息应指明未找到：{error}"
    );
    assert!(
        error.contains("axon sessions list"),
        "错误信息应给出下一步：{error}"
    );
}

#[test]
fn empty_id_is_rejected() {
    let (_directory, config_path) = fixture();
    let error = resolve_session_target(&config_path, "   ").expect_err("空 ID 必须拒绝");
    assert!(error.contains("不能为空"), "{error}");
}

#[test]
fn run_window_lane_detection_distinguishes_owned_windows() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let lane = session("session-window-owner");
    let record = store
        .create_session(&lane, "/workspace", 1)
        .expect("登记车道会话");

    assert!(run_window_has_lane(&store, &record.current_run_id));
    assert!(!run_window_has_lane(&store, "thread-without-lane"));
}
