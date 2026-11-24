//! Thread 派生索引契约：写入即可读、删除写 tombstone、rebuild 与 append 同路径。

use std::path::PathBuf;

use crate::{
    JsonlThreadEventStore, StartEmptyThreadCommand, ThreadService, ThreadSettings, ThreadSource,
};

fn temp_root(tag: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-index-{tag}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

fn start(root: &std::path::Path, thread_id: &str, parent: Option<&str>, created_at_ms: u64) {
    ThreadService::new(JsonlThreadEventStore)
        .start_empty_thread(
            root,
            StartEmptyThreadCommand {
                thread_id: thread_id.to_owned(),
                session_root_id: parent.unwrap_or(thread_id).to_owned(),
                parent_thread_id: parent.map(ToOwned::to_owned),
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: ".".to_owned(),
                    provider: "fixture".to_owned(),
                    model: "fixture-chat".to_owned(),
                    selected_skill_name: None,
                },
                created_at_ms,
            },
        )
        .unwrap();
}

#[test]
fn index_is_readable_after_append_and_removed_on_delete() {
    let root = temp_root("roundtrip");
    start(&root, "root-session", None, 100);
    start(
        &root,
        "root-session--subagent--a",
        Some("root-session"),
        200,
    );
    let store = JsonlThreadEventStore;
    let snapshot = store
        .read_thread_index(&root)
        .unwrap()
        .expect("写入后索引必须可读");
    assert_eq!(snapshot.entries.len(), 2);
    assert_eq!(snapshot.max_updated_at_ms, 200);
    let child = snapshot
        .entries
        .iter()
        .find(|entry| entry.thread_id == "root-session--subagent--a")
        .expect("子会话必须在索引里");
    assert_eq!(child.parent_thread_id.as_deref(), Some("root-session"));
    assert_eq!(child.provider, "fixture");

    assert!(
        store
            .remove_thread(&root, "root-session--subagent--a")
            .unwrap()
    );
    let after = store.read_thread_index(&root).unwrap().expect("索引仍在");
    assert_eq!(after.entries.len(), 1, "删除会话必须同步移除索引条目");
    assert!(
        after
            .entries
            .iter()
            .all(|entry| entry.thread_id != "root-session--subagent--a")
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rebuild_reproduces_the_same_index() {
    let root = temp_root("rebuild");
    start(&root, "root-session", None, 100);
    start(
        &root,
        "root-session--subagent--b",
        Some("root-session"),
        200,
    );
    let store = JsonlThreadEventStore;
    let before = store.read_thread_index(&root).unwrap().expect("索引存在");
    store.rebuild_thread_index(&root).unwrap();
    let after = store.read_thread_index(&root).unwrap().expect("重建后可读");
    assert_eq!(after.entries.len(), before.entries.len());
    assert_eq!(after.max_updated_at_ms, before.max_updated_at_ms);
    let _ = std::fs::remove_dir_all(root);
}
