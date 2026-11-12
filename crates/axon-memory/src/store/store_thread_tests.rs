use super::*;
use crate::{
    COMMITMENT_STATUS_DONE, COMMITMENT_STATUS_OPEN, MEMORY_SQLITE_SCHEMA_VERSION, MemoryConfig,
    MemorySqliteStore, THREAD_STATUS_OPEN,
};

fn temp_store(label: &str) -> (std::path::PathBuf, MemorySqliteStore) {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-thread-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config = MemoryConfig::from_config_dir(
        &root,
        false,
        crate::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    )
    .unwrap();
    std::fs::create_dir_all(config.db_path.parent().unwrap()).unwrap();
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    (root, store)
}

fn thread(id: &str, hit_count: i64, last_event_at_ms: i64) -> MemoryThread {
    MemoryThread {
        thread_id: id.to_owned(),
        namespace_ref: "workspace:current".to_owned(),
        user_ref: "user:local".to_owned(),
        topic: "周报".to_owned(),
        signature: String::new(),
        label: String::new(),
        summary: String::new(),
        status: THREAD_STATUS_OPEN.to_owned(),
        hit_count,
        last_event_at_ms,
        created_at_ms: 1,
        updated_at_ms: last_event_at_ms,
    }
}

fn commitment(
    id: &str,
    thread_id: Option<&str>,
    text: &str,
    created_at_ms: i64,
) -> MemoryCommitment {
    MemoryCommitment {
        commitment_id: id.to_owned(),
        thread_id: thread_id.map(str::to_owned),
        namespace_ref: "workspace:current".to_owned(),
        user_ref: "user:local".to_owned(),
        text: text.to_owned(),
        status: COMMITMENT_STATUS_OPEN.to_owned(),
        channel: "cli".to_owned(),
        created_at_ms,
        closed_at_ms: None,
    }
}

#[test]
fn empty_database_creates_commitment_and_thread_tables() {
    let (_root, store) = temp_store("create");
    assert_eq!(store.commitment_count().unwrap(), 0);
    assert_eq!(store.thread_count().unwrap(), 0);
    let version = store.schema_version().unwrap();
    assert_eq!(version, MEMORY_SQLITE_SCHEMA_VERSION);
    assert_eq!(version, 5, "M3-1 要求 user_version 递增到 5");
}

#[test]
fn legacy_database_keeps_memory_rows_after_upgrade() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-thread-legacy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config = MemoryConfig::from_config_dir(
        &root,
        false,
        crate::MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    )
    .unwrap();
    std::fs::create_dir_all(config.db_path.parent().unwrap()).unwrap();

    let first = MemorySqliteStore::open(&config).unwrap();
    first.initialize_schema().unwrap();
    first
        .connection
        .execute_batch(
            "INSERT INTO memory_records (
                memory_id, candidate_id, memory_kind, state, content_ref, created_at_ms,
                updated_at_ms, last_active_at_ms, confidence_basis_points, importance_basis_points
             ) VALUES (1, 1, 'knowledge', 'strong', 'memory://a', 1, 1, 1, 8000, 8000),
                      (2, 2, 'knowledge', 'strong', 'memory://b', 1, 1, 1, 8000, 8000);
             PRAGMA user_version = 4;",
        )
        .unwrap();
    drop(first);

    let upgraded = MemorySqliteStore::open(&config).unwrap();
    upgraded.initialize_schema().unwrap();
    assert_eq!(upgraded.summary().unwrap().total_records, 2, "升级不得丢行");
    assert_eq!(upgraded.schema_version().unwrap(), 5);
    assert_eq!(upgraded.thread_count().unwrap(), 0);
}

#[test]
fn foreground_thread_prefers_latest_event() {
    let (_root, store) = temp_store("fg");
    store.upsert_thread(&thread("t-old", 1, 10)).unwrap();
    store.upsert_thread(&thread("t-new", 1, 20)).unwrap();
    let fg = store
        .foreground_thread("workspace:current", "user:local")
        .unwrap()
        .expect("应存在前台线索");
    assert_eq!(fg.thread_id, "t-new");
}

#[test]
fn touch_thread_increments_hit_count() {
    let (_root, store) = temp_store("touch");
    store.upsert_thread(&thread("t-1", 1, 10)).unwrap();
    assert!(store.touch_thread("t-1", 30).unwrap());
    let fg = store
        .foreground_thread("workspace:current", "user:local")
        .unwrap()
        .expect("应存在前台线索");
    assert_eq!(fg.hit_count, 2);
    assert_eq!(fg.last_event_at_ms, 30);
}

#[test]
fn archived_thread_is_not_foreground_anymore() {
    let (_root, store) = temp_store("archive");
    store.upsert_thread(&thread("t-1", 1, 10)).unwrap();
    assert!(store.archive_thread("t-1", 40).unwrap());
    assert!(
        store
            .foreground_thread("workspace:current", "user:local")
            .unwrap()
            .is_none()
    );
}

#[test]
fn open_commitment_is_singleton_per_thread() {
    let (_root, store) = temp_store("commitment-singleton");
    store.upsert_thread(&thread("t-1", 1, 10)).unwrap();
    store
        .upsert_open_commitment(&commitment("c-1", Some("t-1"), "明天提醒我交周报", 100))
        .unwrap();
    store
        .upsert_open_commitment(&commitment(
            "c-2",
            Some("t-1"),
            "明天提醒我交周报和日报",
            200,
        ))
        .unwrap();
    assert_eq!(
        store.commitment_count().unwrap(),
        1,
        "同一线索只保留一条 open 承诺"
    );
    let open = store
        .open_commitments("workspace:current", "user:local", 5)
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].commitment_id, "c-1");
    assert_eq!(open[0].text, "明天提醒我交周报和日报");
}

#[test]
fn closed_commitment_leaves_open_list() {
    let (_root, store) = temp_store("commitment-close");
    store
        .upsert_open_commitment(&commitment("c-1", None, "明天提醒我交周报", 100))
        .unwrap();
    assert!(
        store
            .close_commitment("c-1", COMMITMENT_STATUS_DONE, 200)
            .unwrap()
    );
    assert!(
        store
            .open_commitments("workspace:current", "user:local", 5)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.commitment_count().unwrap(), 1, "关闭不得删除行");
}

#[test]
fn open_commitments_are_ordered_by_created_at_desc() {
    let (_root, store) = temp_store("commitment-order");
    store
        .upsert_open_commitment(&commitment("c-1", None, "第一条", 100))
        .unwrap();
    store
        .upsert_open_commitment(&commitment("c-2", None, "第二条", 200))
        .unwrap();
    let open = store
        .open_commitments("workspace:current", "user:local", 5)
        .unwrap();
    assert_eq!(
        open.iter()
            .map(|item| item.commitment_id.as_str())
            .collect::<Vec<_>>(),
        vec!["c-2", "c-1"]
    );
}
