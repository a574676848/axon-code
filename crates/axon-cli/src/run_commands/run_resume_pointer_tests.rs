//! 恢复指针的读写与还原语义。

use std::path::PathBuf;

use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore as _, SqliteRuntimeStore};
use axon_test_support::test_temp_dir;

use super::{restore_resume_session, select_resume_session, snapshot_resume_session};

fn fixture(name: &str) -> (axon_test_support::TestTempDir, PathBuf) {
    let directory = test_temp_dir();
    let config_path = directory.path().join("config.toml");
    std::fs::write(&config_path, "").expect("配置应可写入");
    let data_root = directory.path().join(axon_config::AXON_RUNTIME_DATA_DIR);
    let store = SqliteRuntimeStore::from_root(data_root);
    store.initialize().expect("schema 应可初始化");
    store
        .create_session(
            &DaemonSessionId::new(format!("session-{name}")).expect("合法会话 ID"),
            "/workspace",
            1,
        )
        .expect("会话应可创建");
    store
        .create_session(
            &DaemonSessionId::new(format!("session-{name}-other")).expect("合法会话 ID"),
            "/workspace",
            1,
        )
        .expect("会话应可创建");
    (directory, config_path)
}

#[test]
fn select_then_snapshot_reads_back_the_same_session() {
    let (_root, config_path) = fixture("resume-select");
    select_resume_session(&config_path, "session-resume-select").expect("选择应成功");
    assert_eq!(
        snapshot_resume_session(&config_path)
            .expect("快照应成功")
            .as_deref(),
        Some("session-resume-select")
    );
}

#[test]
fn selecting_unknown_session_is_rejected() {
    let (_root, config_path) = fixture("resume-missing");
    let error = select_resume_session(&config_path, "session-does-not-exist")
        .expect_err("不存在的会话必须拒绝");
    assert!(error.contains("会话不存在"), "{error}");
}

#[test]
fn restore_brings_back_the_previous_pointer() {
    let (_root, config_path) = fixture("resume-restore");
    select_resume_session(&config_path, "session-resume-restore").expect("选择应成功");
    let snapshot = snapshot_resume_session(&config_path).expect("快照应成功");
    select_resume_session(&config_path, "session-resume-restore-other").expect("改写应成功");
    restore_resume_session(&config_path, snapshot.as_deref()).expect("还原应成功");
    assert_eq!(
        snapshot_resume_session(&config_path)
            .expect("快照应成功")
            .as_deref(),
        Some("session-resume-restore")
    );
}

#[test]
fn restore_with_none_clears_the_pointer() {
    let (_root, config_path) = fixture("resume-clear");
    select_resume_session(&config_path, "session-resume-clear").expect("选择应成功");
    restore_resume_session(&config_path, None).expect("清空应成功");
    assert_eq!(
        snapshot_resume_session(&config_path).expect("快照应成功"),
        None
    );
}
