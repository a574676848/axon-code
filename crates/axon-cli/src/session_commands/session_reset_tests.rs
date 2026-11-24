//! `axon sessions reset` 测试：验收「保留会话、只换运行窗口」这一条语义。

use std::path::PathBuf;

use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore, SqliteRuntimeStore};

use super::{SessionResetCommand, run_session_reset};
use crate::OutputFormat;

struct Fixture {
    _directory: axon_test_support::TestTempDir,
    config_path: PathBuf,
    store: SqliteRuntimeStore,
    session_id: DaemonSessionId,
}

fn fixture() -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    let store = SqliteRuntimeStore::from_root(directory.path().join("data"));
    store.initialize().expect("初始化运行态库");
    let session_id = DaemonSessionId::new("session-reset-target".to_owned()).expect("合法会话 ID");
    store
        .create_session(&session_id, "/workspace", 1)
        .expect("登记会话");
    Fixture {
        _directory: directory,
        config_path,
        store,
        session_id,
    }
}

#[test]
fn reset_rotates_the_run_window_and_keeps_the_session() {
    let fixture = fixture();
    let before = fixture
        .store
        .get_session(&fixture.session_id)
        .expect("读取会话");
    let result = run_session_reset(
        OutputFormat::Json,
        &fixture.config_path,
        &SessionResetCommand {
            session_id: fixture.session_id.as_str().to_owned(),
        },
    );
    assert_eq!(result.exit_code, 0, "reset 应成功：{}", result.stderr);
    assert!(result.stdout.contains("session_reset"), "{}", result.stdout);

    let after = fixture
        .store
        .get_session(&fixture.session_id)
        .expect("读取会话");
    assert_eq!(after.session_id, before.session_id, "会话本身不变");
    assert_ne!(
        after.current_run_id, before.current_run_id,
        "运行窗口必须换成新的一段"
    );
}

#[test]
fn reset_rejects_unknown_session() {
    let fixture = fixture();
    let result = run_session_reset(
        OutputFormat::Text,
        &fixture.config_path,
        &SessionResetCommand {
            session_id: "session-does-not-exist".to_owned(),
        },
    );
    assert_ne!(result.exit_code, 0);
    assert!(result.stderr.contains("未找到会话"), "{}", result.stderr);
}
