//! 子代理来源标注测试：标注落在 thread 事件面的索引里，重复标注以最后一次为准。

use std::path::{Path, PathBuf};

use axon_session::{
    JsonlThreadEventStore, StartEmptyThreadCommand, ThreadService, ThreadSettings, ThreadSource,
};

use super::{CHILD_SESSION_ORIGIN_DELEGATED, CHILD_SESSION_ORIGIN_INTERNAL, record_child_origin};

fn temp_root(tag: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-origin-{tag}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

/// 建一条空 thread：来源标注写在 thread 事件面上，索引条目要先有这条 thread。
fn seed_thread(root: &Path, thread_id: &str) {
    ThreadService::new(JsonlThreadEventStore)
        .start_empty_thread(
            root,
            StartEmptyThreadCommand {
                thread_id: thread_id.to_owned(),
                session_root_id: thread_id.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture-chat".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms: 100,
            },
        )
        .expect("建 thread");
}

/// 读出某条 thread 的已标注来源。
fn origin_of(root: &Path, thread_id: &str) -> Option<String> {
    let snapshot = JsonlThreadEventStore
        .read_thread_index(root)
        .expect("读索引")
        .expect("索引存在");
    snapshot
        .entries
        .iter()
        .find(|entry| entry.thread_id == thread_id)
        .and_then(|entry| entry.child_origin.clone())
}

#[test]
fn delegated_origin_lands_on_the_thread_index() {
    let root = temp_root("delegated");
    seed_thread(&root, "run-a--subagent--delegate");
    record_child_origin(
        &root.join("config.toml"),
        "run-a--subagent--delegate",
        CHILD_SESSION_ORIGIN_DELEGATED,
    )
    .expect("标注");
    assert_eq!(
        origin_of(&root, "run-a--subagent--delegate").as_deref(),
        Some(CHILD_SESSION_ORIGIN_DELEGATED)
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn internal_origin_overwrites_an_earlier_annotation() {
    let root = temp_root("overwrite");
    seed_thread(&root, "run-b--subagent--salience");
    let config = root.join("config.toml");
    record_child_origin(
        &config,
        "run-b--subagent--salience",
        CHILD_SESSION_ORIGIN_DELEGATED,
    )
    .expect("首次标注");
    record_child_origin(
        &config,
        "run-b--subagent--salience",
        CHILD_SESSION_ORIGIN_INTERNAL,
    )
    .expect("覆盖标注");
    assert_eq!(
        origin_of(&root, "run-b--subagent--salience").as_deref(),
        Some(CHILD_SESSION_ORIGIN_INTERNAL),
        "重复标注必须以最后一次为准"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn empty_session_id_is_rejected() {
    let root = temp_root("empty");
    let error = record_child_origin(
        &root.join("config.toml"),
        "   ",
        CHILD_SESSION_ORIGIN_DELEGATED,
    )
    .expect_err("空标识必须拒绝");
    assert!(error.contains("不能为空"), "实际错误：{error}");
    let _ = std::fs::remove_dir_all(root);
}
