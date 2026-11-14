//! rollout 历史窗口、checkpoint 与 resume 链回放测试。

use super::super::RolloutSqliteStore;
use super::{cleanup, record, temp_db};
use axon_session::ThreadCompaction;

#[test]
fn history_window_starts_after_checkpoint_without_deleting_raw_turns() {
    let path = temp_db("history-window");
    let store = RolloutSqliteStore::open(&path).unwrap();
    for (sequence, prompt) in ["first", "second", "third"].into_iter().enumerate() {
        let mut turn = record("s1", None, u64::try_from(sequence).unwrap());
        turn.assistant_part_id = format!("assistant:s1:{sequence}");
        turn.prompt = prompt.to_owned();
        store.record_session(&turn).unwrap();
    }
    let checkpoint = ThreadCompaction {
        history_json: serde_json::json!([
            {"role": "user", "content": "checkpoint", "cache_excluded": true}
        ])
        .to_string(),
        through_sequence: 1,
        checkpoint_text: "checkpoint".to_owned(),
        before_tokens: 100,
        after_tokens: 10,
        recorded_at_ms: 10,
    };
    store.install_compaction("s1", &checkpoint).unwrap();

    let window = store.load_history_window("s1").unwrap();
    assert_eq!(window.compaction, Some(checkpoint));
    assert_eq!(window.turns.len(), 1);
    assert_eq!(window.turns[0].0, 2);
    assert_eq!(window.turns[0].1.prompt, "third");
    let raw_turn_count = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM rollout_turns WHERE session_id = 's1'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(raw_turn_count, 3);
    cleanup(&path);
}

#[test]
fn replace_compaction_can_restore_previous_rollout_checkpoint() {
    let path = temp_db("restore-compaction");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&record("s1", None, 1)).unwrap();
    let checkpoint = ThreadCompaction {
        history_json: "[]".to_owned(),
        through_sequence: 0,
        checkpoint_text: "summary".to_owned(),
        before_tokens: 10,
        after_tokens: 5,
        recorded_at_ms: 2,
    };

    store.install_compaction("s1", &checkpoint).unwrap();
    store.replace_compaction("s1", None).unwrap();

    assert!(
        store
            .load_history_window("s1")
            .unwrap()
            .compaction
            .is_none()
    );
    cleanup(&path);
}

#[test]
fn replay_chain_returns_root_first_order() {
    let path = temp_db("chain");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&record("root", None, 100)).unwrap();
    store
        .record_session(&record("mid", Some("root"), 200))
        .unwrap();
    store
        .record_session(&record("leaf", Some("mid"), 300))
        .unwrap();

    let chain = store.replay_chain("leaf").unwrap();
    let ids: Vec<&str> = chain.iter().map(|r| r.session_id.as_str()).collect();
    assert_eq!(ids, vec!["root", "mid", "leaf"]);
    cleanup(&path);
}

#[test]
fn replay_chain_handles_missing_parent_gracefully() {
    let path = temp_db("orphan");
    let store = RolloutSqliteStore::open(&path).unwrap();
    // parent 指向不存在的会话，回放应止于已存在的链段。
    store
        .record_session(&record("child", Some("ghost"), 100))
        .unwrap();

    let chain = store.replay_chain("child").unwrap();
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].session_id, "child");
    cleanup(&path);
}

#[test]
fn replay_chain_detects_cycle_without_depth_limit() {
    let path = temp_db("cycle");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&record("root", None, 100)).unwrap();
    store
        .record_session(&record("child", Some("root"), 200))
        .unwrap();
    store
        .record_session(&record("root", Some("child"), 300))
        .unwrap();

    assert_eq!(
        store.replay_chain("child").unwrap_err(),
        "rollout resume 链存在环"
    );
    cleanup(&path);
}

#[test]
fn list_sessions_orders_by_created_at_desc() {
    let path = temp_db("list");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&record("a", None, 100)).unwrap();
    store.record_session(&record("b", None, 300)).unwrap();
    store.record_session(&record("c", None, 200)).unwrap();

    let summaries = store.list_sessions().unwrap();
    let ids: Vec<&str> = summaries.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec!["b", "c", "a"]);
    assert_eq!(store.load_latest().unwrap().unwrap().session_id, "b");
    cleanup(&path);
}
