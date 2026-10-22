use super::*;

#[test]
fn resolved_rollout_path_uses_data_sessions_directory() {
    let config_path = temp_config("data-directory");
    let root = config_path.parent().expect("配置文件应有父目录");
    let path = resolved_rollout_db_path(&config_path).unwrap();

    assert_eq!(
        path,
        root.join(AXON_RUNTIME_DATA_DIR)
            .join(SESSION_STORE_DIR)
            .join(CLI_ROLLOUT_DB_FILE)
    );
    let _store = RolloutSqliteStore::open(&path).unwrap();
    assert!(path.exists());
    assert!(!root.join(SESSION_STORE_DIR).exists());
}
