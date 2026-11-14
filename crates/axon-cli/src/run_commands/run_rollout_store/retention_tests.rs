//! rollout 会话家族保留策略与工具输出读取范围测试。

use std::collections::HashSet;

use super::super::{
    RolloutSqliteStore, read_rollout_tool_output, replay_session_chain, resolved_rollout_db_path,
};
use super::{cleanup, record, temp_config, temp_db};

#[test]
fn stale_session_family_is_removed_but_current_family_is_preserved() {
    let path = temp_db("stale-family-cleanup");
    let mut store = RolloutSqliteStore::open(&path).unwrap();
    store
        .record_session(&record("old-root", None, 100))
        .unwrap();
    store
        .record_session(&record("old-root::child", Some("old-root"), 200))
        .unwrap();
    store
        .record_session(&record("current-root", None, 1_000))
        .unwrap();

    let candidates = store
        .stale_session_family_ids(500, &HashSet::new())
        .unwrap();

    assert_eq!(
        candidates,
        vec!["old-root".to_owned(), "old-root::child".to_owned()]
    );
    assert!(store.load_by_id("old-root").unwrap().is_some());
    assert!(store.load_by_id("old-root::child").unwrap().is_some());

    let report = store
        .finalize_stale_session_family_removal(&candidates, 500, &HashSet::new())
        .unwrap();

    assert_eq!(report.session_ids, candidates);
    assert!(store.load_by_id("old-root").unwrap().is_none());
    cleanup(&path);
}

#[test]
fn protected_session_family_is_not_removed_by_retention_cleanup() {
    let path = temp_db("protected-family-cleanup");
    let mut store = RolloutSqliteStore::open(&path).unwrap();
    store
        .record_session(&record("old-root", None, 100))
        .unwrap();
    store
        .record_session(&record("old-root::child", Some("old-root"), 200))
        .unwrap();
    store
        .record_session(&record("current-root", None, 1_000))
        .unwrap();

    let candidates = store
        .stale_session_family_ids(500, &HashSet::new())
        .unwrap();
    let protected = HashSet::from(["old-root::child".to_owned()]);
    let report = store
        .finalize_stale_session_family_removal(&candidates, 500, &protected)
        .unwrap();

    assert!(report.session_ids.is_empty());
    assert!(store.load_by_id("old-root").unwrap().is_some());
    assert!(store.load_by_id("old-root::child").unwrap().is_some());
    cleanup(&path);
}

#[test]
fn persisted_tool_output_is_read_only_with_session_receipt_scope_after_resume_and_replay() {
    let config_path = temp_config("tool-output-read-scope");
    let path = resolved_rollout_db_path(&config_path).unwrap();
    let archive_root = crate::runtime_paths::runtime_axon_data_path_static(&config_path)
        .expect("测试 Axon data 目录应可解析");
    let archive = axon_agent_runtime::ContentArchiveRuntime::new(1);
    let output_ref = archive
        .archive_text(
            &archive_root,
            axon_agent_runtime::CONTENT_ARCHIVE_RESULT_STREAM,
            "完整归档正文",
            "builtin/shell_command",
        )
        .unwrap()
        .output_ref
        .expect("应生成 OutputRef");

    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut root = record("root", None, 100);
    root.tool_receipts = vec![crate::run_commands::run_tool_loop::RunToolReceipt {
        tool_call_id: 1,
        part_id: "tool-part:root".to_owned(),
        provider_call_id: "provider-call-root".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        status: "completed".to_owned(),
        summary: "完整归档正文".to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: Some(output_ref.clone()),
        duration_ms: Some(1),
    }];
    store.record_session(&root).unwrap();

    let resumed = store
        .load_by_id("root")
        .unwrap()
        .expect("resume 应加载已持久化 session");
    assert_eq!(
        resumed.tool_receipts[0].output_ref.as_ref(),
        Some(&output_ref),
        "resume 应保留 OutputRef"
    );
    drop(store);
    assert_eq!(
        read_rollout_tool_output(&config_path, "root", "provider-call-root").unwrap(),
        "完整归档正文".as_bytes()
    );

    let store = RolloutSqliteStore::open(&path).unwrap();
    let child = record("root::child", Some("root"), 200);
    store.record_session(&child).unwrap();
    drop(store);
    let replayed = replay_session_chain(&config_path, "root::child").unwrap();
    assert_eq!(replayed.len(), 2, "replay 应保留完整 resume 链");
    assert_eq!(
        read_rollout_tool_output(&config_path, "root", "provider-call-root").unwrap(),
        "完整归档正文".as_bytes()
    );
    assert!(read_rollout_tool_output(&config_path, "root::child", "provider-call-root").is_err());

    let archive_path = archive_root.join(
        output_ref
            .archive_ref
            .as_ref()
            .expect("OutputRef 应保存相对归档路径")
            .relative_path
            .clone(),
    );
    std::fs::write(archive_path, "被篡改的归档正文").unwrap();
    assert!(read_rollout_tool_output(&config_path, "root", "provider-call-root").is_err());
    let _ = std::fs::remove_dir_all(archive_root);
}
