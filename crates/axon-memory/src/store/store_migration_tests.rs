use super::*;

/// 旧库没有可信 `memory_kind` 来源，因此不能猜测填充；但既有记录属于用户数据，
/// 升级必须只归档不删除。这条测试锁死“升级不丢行”的契约。
#[test]
fn incompatible_legacy_table_is_archived_without_losing_rows() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-schema-v1-{}-{}",
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
    let connection = Connection::open(&config.db_path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE memory_records (
                memory_id INTEGER PRIMARY KEY NOT NULL,
                source_summary_hash TEXT
            );
            CREATE INDEX idx_memory_records_active_score ON memory_records(source_summary_hash);
            INSERT INTO memory_records(memory_id, source_summary_hash) VALUES (7, 'legacy-a'), (8, 'legacy-b');
            PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);

    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();

    assert_eq!(
        store.schema_version().unwrap(),
        MEMORY_SQLITE_SCHEMA_VERSION
    );
    let columns = store.record_table_columns().unwrap();
    assert!(
        REQUIRED_MEMORY_RECORD_COLUMNS
            .iter()
            .all(|column| columns.contains(*column))
    );
    assert_eq!(
        store.summary().unwrap().total_records,
        0,
        "新表必须是干净的目标态，不能把不可信旧行混入"
    );

    let archived = store.legacy_record_tables().unwrap();
    assert_eq!(archived.len(), 1, "旧表必须被归档：{archived:?}");
    let preserved: i64 = store
        .connection
        .query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", archived[0]),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(preserved, 2, "归档表必须保留全部旧行");

    let supersession_table: String = store
        .connection
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'memory_supersessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(supersession_table, "memory_supersessions");

    // 归档表继承了旧索引名，必须释放，否则新表的同名索引会被 IF NOT EXISTS 静默跳过。
    let canonical_indexes: Vec<String> = store.table_index_names("memory_records").unwrap();
    assert!(
        canonical_indexes
            .iter()
            .any(|name| name == "idx_memory_records_kind_timeline"),
        "新表必须真正建出类型时间轴索引：{canonical_indexes:?}"
    );
    assert!(
        store
            .table_index_names(&archived[0])
            .unwrap()
            .iter()
            .all(|name| name != "idx_memory_records_active_score"),
        "旧索引名必须从归档表释放"
    );

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}

/// 同一个 memory.db 由多个 store 共用：`user_version` 只能归 memory_records 所有，
/// 向量投影必须把版本写到自己的组件表里，否则真机会把 v4 库读成 2。
#[test]
fn shared_database_keeps_record_schema_version_authoritative() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-shared-schema-{}-{}",
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
    let store = MemorySqliteStore::open(&config).unwrap();
    store.initialize_schema().unwrap();
    let projections = crate::MemoryVectorProjectionStore::open(&config).unwrap();
    projections.initialize_schema().unwrap();

    assert_eq!(
        store.schema_version().unwrap(),
        MEMORY_SQLITE_SCHEMA_VERSION,
        "向量投影初始化不得覆盖 memory_records 的 schema 版本"
    );
    assert_eq!(
        projections.schema_version().unwrap(),
        Some(crate::MEMORY_VECTOR_PROJECTION_SCHEMA_VERSION)
    );

    drop(projections);
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}
