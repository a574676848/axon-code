use super::*;
use crate::{MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig};

fn temp_store(label: &str) -> MemoryRuntimeQualityStore {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-quality-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config =
        MemoryConfig::from_config_dir(&root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap();
    std::fs::create_dir_all(config.db_path.parent().unwrap()).unwrap();
    let store = MemoryRuntimeQualityStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    store
}

#[test]
fn fresh_schema_has_consolidation_columns() {
    let store = temp_store("fresh");
    assert_eq!(store.consolidation_stats().unwrap(), (0, 0));
}

#[test]
fn consolidation_stats_roundtrip() {
    let store = temp_store("roundtrip");
    store.record_consolidation_stats(16, 3, 1_000).unwrap();
    store.record_consolidation_stats(4, 1, 2_000).unwrap();
    assert_eq!(store.consolidation_stats().unwrap(), (20, 4));
}

#[test]
fn same_timestamp_is_idempotent() {
    let store = temp_store("idempotent");
    store.record_consolidation_stats(16, 3, 1_000).unwrap();
    store.record_consolidation_stats(16, 3, 1_000).unwrap();
    assert_eq!(store.consolidation_stats().unwrap(), (16, 3));
}

#[test]
fn invalid_timestamp_is_rejected() {
    let store = temp_store("invalid");
    assert!(store.record_consolidation_stats(1, 1, 0).is_err());
}

#[test]
fn legacy_table_gets_columns_added() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-quality-legacy-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config =
        MemoryConfig::from_config_dir(&root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap();
    std::fs::create_dir_all(config.db_path.parent().unwrap()).unwrap();
    let connection = Connection::open(&config.db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE memory_runtime_quality (
                observation_id TEXT PRIMARY KEY,
                cache_read_input_tokens INTEGER NOT NULL,
                complete_input_tokens INTEGER NOT NULL,
                reflection_attempts INTEGER NOT NULL,
                reflection_yields INTEGER NOT NULL,
                semantic_assessments INTEGER NOT NULL,
                false_memory_rejections INTEGER NOT NULL,
                recorded_at_ms INTEGER NOT NULL
            );
            INSERT INTO memory_runtime_quality VALUES ('legacy', 10, 20, 1, 1, 2, 0, 500);",
        )
        .unwrap();
    drop(connection);

    let store = MemoryRuntimeQualityStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    assert_eq!(store.consolidation_stats().unwrap(), (0, 0));
    store.record_consolidation_stats(9, 2, 1_000).unwrap();
    assert_eq!(store.consolidation_stats().unwrap(), (9, 2));
    let legacy_rows: i64 = store
        .connection
        .query_row(
            "SELECT COUNT(*) FROM memory_runtime_quality WHERE observation_id = 'legacy'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy_rows, 1, "补列不得丢旧观测行");
}
