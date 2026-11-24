//! 会话标识与运行窗口标识的解析测试。
//!
//! 关键口径：两个标识空间互不回落。`axon sessions` 只认会话标识（ULID，解析出运行窗口），
//! `axon debug session` 只认运行窗口标识（解析出 Thread 本身与它归属的会话）。

use std::path::{Path, PathBuf};

use axon_domain::DaemonSessionId;
use axon_session::{
    JsonlThreadEventStore, RuntimeStore, SqliteRuntimeStore, StartEmptyThreadCommand,
    ThreadService, ThreadSettings, ThreadSource,
};
use rusqlite::params;

use super::{TargetKind, resolve_session_target};

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
fn session_target_resolves_to_its_run_window() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let target_session = session("01ARZ3NDEKTSV4RRFFQ69G5FAV");
    let record = store
        .create_session(&target_session, "/workspace", 1)
        .expect("登记会话");
    seed_thread(&config_path, &record.current_run_id);

    let resolved =
        resolve_session_target(&config_path, TargetKind::Session, target_session.as_str())
            .expect("会话 ID 应可解析");

    assert_eq!(resolved.thread_id, record.current_run_id);
    assert_eq!(
        resolved.session_id.as_deref(),
        Some(target_session.as_str())
    );
}

#[test]
fn thread_target_keeps_thread_identity_and_reports_owning_session() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let target_session = session("01ARZ3NDEKTSV4RRFFQ69G5FAW");
    let record = store
        .create_session(&target_session, "/workspace", 1)
        .expect("登记会话");
    seed_thread(&config_path, &record.current_run_id);

    let resolved = resolve_session_target(&config_path, TargetKind::Thread, &record.current_run_id)
        .expect("Thread ID 应可解析");

    assert_eq!(resolved.thread_id, record.current_run_id);
    assert_eq!(
        resolved.session_id.as_deref(),
        Some(target_session.as_str())
    );
}

#[test]
fn thread_target_without_session_row_keeps_no_session() {
    let (_directory, config_path) = fixture();
    let thread_id = "fork-thread-without-session";
    seed_thread(&config_path, thread_id);

    let resolved = resolve_session_target(&config_path, TargetKind::Thread, thread_id)
        .expect("未登记会话的 Thread 仍应可解析");

    assert_eq!(resolved.thread_id, thread_id);
    assert_eq!(resolved.session_id, None, "没有归属行时不得伪造会话标识");
}

#[test]
fn session_target_does_not_fall_back_to_thread_id() {
    let (_directory, config_path) = fixture();
    let thread_id = "thread-without-session-row";
    seed_thread(&config_path, thread_id);

    let error = resolve_session_target(&config_path, TargetKind::Session, thread_id)
        .expect_err("会话入口不得接受 Thread 标识");

    assert!(error.contains("未找到会话"), "{error}");
}

#[test]
fn thread_target_does_not_fall_back_to_session_id() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let target_session = session("01ARZ3NDEKTSV4RRFFQ69G5FAX");
    store
        .create_session(&target_session, "/workspace", 1)
        .expect("登记会话");

    let error = resolve_session_target(&config_path, TargetKind::Thread, target_session.as_str())
        .expect_err("Thread 入口不得接受会话标识");

    assert!(error.contains("未找到 Thread"), "{error}");
}

#[test]
fn unknown_id_reports_not_found() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    store
        .create_session(&session("01ARZ3NDEKTSV4RRFFQ69G5FAY"), "/workspace", 1)
        .expect("登记会话");

    let error = resolve_session_target(
        &config_path,
        TargetKind::Session,
        "01ARZ3NDEKTSV4RRFFQ69G5FAY",
    )
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
    let error = resolve_session_target(&config_path, TargetKind::Session, "   ")
        .expect_err("空 ID 必须拒绝");
    assert!(error.contains("不能为空"), "{error}");
}

#[test]
fn thread_target_reads_the_session_column_not_a_prefix() {
    let (_directory, config_path) = fixture();
    let store = store_for(&config_path);
    let owner = session("01ARZ3NDEKTSV4RRFFQ69G5FB0");
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO sessions(session_id,current_run_id,root_session_id,trust_scope,kind,cwd,status,
                         worker_attempt,context_version,last_checkpoint_seq,last_event_sequence,
                         last_activity_at_ms,created_at_ms,updated_at_ms)
                     VALUES(?1,?2,NULL,'private','primary','/workspace','created',0,0,0,0,5,5,5)",
                    params![owner.as_str(), "explicit-run-window"],
                )
                .map(|_| ())
        })
        .expect("写入会话行");
    seed_thread(&config_path, "explicit-run-window");

    let resolved = resolve_session_target(&config_path, TargetKind::Thread, "explicit-run-window")
        .expect("Thread 应可解析");

    assert_eq!(resolved.session_id.as_deref(), Some(owner.as_str()));
}
