//! `axon debug session prune --orphans` 测试：只收归属会话已消失的非主会话。

use std::path::PathBuf;

use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore as _, SqliteRuntimeStore};
use rusqlite::params;

use super::SessionPruneCommand;
use super::run_session_prune;
use crate::OutputFormat;

/// 一次清理用例的隔离现场。
struct Fixture {
    _directory: axon_test_support::TestTempDir,
    /// 配置路径：运行根取其所在目录，数据落在其 `data` 子目录。
    config_path: PathBuf,
    store: SqliteRuntimeStore,
}

fn fixture() -> Fixture {
    let directory = axon_test_support::test_temp_dir();
    let config_path = directory.path().join("config.toml");
    let data_root = directory.path().join("data");
    let store = SqliteRuntimeStore::from_root(&data_root);
    store.initialize().expect("初始化运行态库");
    Fixture {
        _directory: directory,
        config_path,
        store,
    }
}

/// 登记一条会话并把类型与归属写死：`create_session` 只造主会话，非主会话靠这两列表达。
fn seed_session(
    store: &SqliteRuntimeStore,
    session_id: &str,
    kind: &str,
    root_session_id: Option<&str>,
) {
    let session = DaemonSessionId::new(session_id.to_owned()).expect("合法会话 ID");
    store
        .create_session(&session, "/workspace", 1)
        .expect("登记会话");
    store
        .with_read_connection(|connection| {
            connection
                .execute(
                    "UPDATE sessions SET kind=?1, root_session_id=?2, status='completed'
                      WHERE session_id=?3",
                    params![kind, root_session_id, session_id],
                )
                .map(|_| ())
        })
        .expect("写入会话类型与归属");
}

fn prune(fixture: &Fixture) -> serde_json::Value {
    let result = run_session_prune(
        OutputFormat::Json,
        &fixture.config_path,
        &SessionPruneCommand { orphans: true },
    );
    assert_eq!(result.exit_code, 0, "清理应当成功：{}", result.stderr);
    serde_json::from_str(&result.stdout).expect("清理回执是 JSON")
}

#[test]
fn orphan_subagent_whose_parent_is_gone_is_removed() {
    let fixture = fixture();
    seed_session(&fixture.store, "session-parent", "primary", None);
    seed_session(
        &fixture.store,
        "session-orphan",
        "subagent",
        Some("session-gone"),
    );

    let report = prune(&fixture);

    assert_eq!(report["status"], "session_pruned");
    assert_eq!(report["scanned"], 1);
    assert_eq!(report["orphaned"], 1);
    assert_eq!(report["deleted"][0]["sessionId"], "session-orphan");
    assert_eq!(report["deleted"][0]["kind"], "subagent");
    // 控制面行确实没了。
    let orphan = DaemonSessionId::new("session-orphan".to_owned()).expect("合法会话 ID");
    assert!(
        fixture.store.get_session(&orphan).is_err(),
        "孤儿会话必须从控制面删除"
    );
    let parent = DaemonSessionId::new("session-parent".to_owned()).expect("合法会话 ID");
    assert!(
        fixture.store.get_session(&parent).is_ok(),
        "归属仍在的会话不能被顺带删掉"
    );
}

#[test]
fn subagent_whose_parent_still_exists_is_left_alone() {
    let fixture = fixture();
    seed_session(&fixture.store, "session-parent", "primary", None);
    seed_session(
        &fixture.store,
        "session-child",
        "subagent",
        Some("session-parent"),
    );

    let report = prune(&fixture);

    assert_eq!(report["deleted"].as_array().map(Vec::len), Some(0));
    assert_eq!(report["skipped"][0]["sessionId"], "session-child");
    assert!(
        report["skipped"][0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("仍然存在")),
        "跳过原因要指出归属仍在：{}",
        report["skipped"][0]["reason"]
    );
    let child = DaemonSessionId::new("session-child".to_owned()).expect("合法会话 ID");
    assert!(
        fixture.store.get_session(&child).is_ok(),
        "归属仍在时不得删除"
    );
}

#[test]
fn prune_refuses_to_run_without_the_orphans_switch() {
    let fixture = fixture();
    seed_session(
        &fixture.store,
        "session-orphan",
        "subagent",
        Some("session-gone"),
    );

    let result = run_session_prune(
        OutputFormat::Text,
        &fixture.config_path,
        &SessionPruneCommand { orphans: false },
    );

    assert_ne!(result.exit_code, 0, "缺少 --orphans 时必须拒绝执行");
    assert!(
        result.stderr.contains("--orphans"),
        "拒绝原因要指名缺的参数：{}",
        result.stderr
    );
}
