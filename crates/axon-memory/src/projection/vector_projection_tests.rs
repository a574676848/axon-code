use super::*;

#[test]
fn projection_lookup_accepts_more_than_one_sql_batch() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-batch-projections-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config = MemoryConfig::from_config_dir(&root, false, 2).unwrap();
    std::fs::create_dir_all(config.db_path.parent().unwrap()).unwrap();
    let store = MemoryVectorProjectionStore::open(&config).unwrap();
    let ids = (1..=MEMORY_VECTOR_PROJECTION_QUERY_BATCH_SIZE + 1)
        .map(|value| value as i64)
        .collect::<Vec<_>>();

    assert!(store.projections_by_vector_ids(&ids).unwrap().is_empty());

    drop(store);
    let _ = std::fs::remove_dir_all(root);
}
