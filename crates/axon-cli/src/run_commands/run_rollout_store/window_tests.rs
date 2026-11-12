use super::*;

fn checkpoint(through_sequence: u64) -> ThreadCompaction {
    ThreadCompaction {
        history_json: "[]".to_owned(),
        through_sequence,
        checkpoint_text: "便签交接".to_owned(),
        before_tokens: 100,
        after_tokens: 10,
        recorded_at_ms: 10,
    }
}

#[test]
fn active_checkpoint_covers_the_current_turn_and_keeps_raw_history() {
    let path = temp_db("active-window");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut old = record("s1", None, 1);
    old.assistant_part_id = "old-turn".to_owned();
    store.commit_session(&old, None).unwrap();
    let mut current = record("s1", None, 2);
    current.assistant_part_id = "current-turn".to_owned();
    store.commit_session(&current, None).unwrap();
    let checkpoint = checkpoint(999);
    store
        .install_active_window("s1", "current-turn", checkpoint)
        .unwrap();
    let window = store.load_history_window("s1").unwrap();
    assert_eq!(window.compaction.unwrap().through_sequence, 1);
    assert!(window.turns.is_empty());
    current.response = "换窗后的最终回复".to_owned();
    store.commit_session(&current, None).unwrap();
    assert_eq!(
        store.load_by_id("s1").unwrap().unwrap().response,
        current.response
    );
    assert!(store.load_history_window("s1").unwrap().turns.is_empty());
    let mut next = record("s1", None, 3);
    next.assistant_part_id = "next-turn".to_owned();
    store.commit_session(&next, None).unwrap();
    let window = store.load_history_window("s1").unwrap();
    assert_eq!(window.turns.len(), 1);
    assert_eq!(window.turns[0].0, 2);
    cleanup(&path);
}

#[test]
fn started_turn_lets_the_active_window_cover_the_current_turn() {
    let path = temp_db("started-active-window");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut earlier = record("s1", None, 1);
    earlier.assistant_part_id = "earlier-turn".to_owned();
    store.commit_session(&earlier, None).unwrap();
    let mut started = record("s1", None, 2);
    started.assistant_part_id = "current-turn".to_owned();
    let sequence = store.ensure_started_turn(&started).unwrap();

    // 起始记录已足以定位当前轮：这条交互的首个 provider turn 触发换窗不会白跑。
    store
        .install_active_window("s1", "current-turn", checkpoint(sequence))
        .expect("起始记录落地后窗口必须能覆盖当前轮");

    // 收尾提交按同一幂等键覆盖成完整记录，sequence 保持稳定。
    started.response = "换窗后的最终回复".to_owned();
    store.commit_session(&started, None).unwrap();
    let window = store.load_history_window("s1").unwrap();
    assert_eq!(window.compaction.unwrap().through_sequence, sequence);
    assert!(window.turns.is_empty());
    cleanup(&path);
}

#[test]
fn missing_current_turn_does_not_replace_an_existing_checkpoint() {
    let path = temp_db("missing-active-window");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.commit_session(&record("s1", None, 1), None).unwrap();
    let original = checkpoint(0);
    store.install_compaction("s1", &original).unwrap();
    assert!(
        store
            .install_active_window("s1", "unknown-turn", checkpoint(1))
            .is_err()
    );
    assert_eq!(
        store.load_history_window("s1").unwrap().compaction,
        Some(original)
    );
    cleanup(&path);
}
