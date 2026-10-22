//! `run_rollout_store` SQLite 结构化回放测试。

#[path = "path_tests.rs"]
mod path_tests;
#[path = "window_tests.rs"]
mod window_tests;

use super::{
    CLI_ROLLOUT_DB_FILE, RolloutEventType, RolloutSqliteStore, RunSessionRecord,
    resolved_rollout_db_path,
};
use axon_agent_runtime::{agent_run_assistant_part_id, agent_run_reasoning_part_id};
use axon_session::{AXON_RUNTIME_DATA_DIR, SESSION_STORE_DIR, ThreadCompaction};
use std::collections::HashSet;

const FIXTURE_ASSISTANT_TURN_ID: u64 = 1;

fn temp_db(tag: &str) -> std::path::PathBuf {
    let mut dir = axon_test_support::test_temp_root();
    let unique = format!(
        "axon_rollout_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    dir.push(unique);
    dir.push("rollout.sqlite3");
    dir
}

fn temp_config(tag: &str) -> std::path::PathBuf {
    temp_db(tag)
        .parent()
        .expect("临时数据库应有父目录")
        .join("config.toml")
}

fn record(session_id: &str, parent: Option<&str>, created_at_ms: u64) -> RunSessionRecord {
    RunSessionRecord {
        session_id: session_id.to_owned(),
        parent_session_id: parent.map(ToOwned::to_owned),
        created_at_ms,
        startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
        cwd: Some("/work".to_owned()),
        provider: "anthropic".to_owned(),
        model: "model-a".to_owned(),
        prompt: format!("prompt-{session_id}"),
        local_images: Vec::new(),
        local_files: Vec::new(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        response: format!("response-{session_id}"),
        assistant_part_id: agent_run_assistant_part_id(FIXTURE_ASSISTANT_TURN_ID),
        usage: None,
        tool_receipts: vec![],
        tool_calls: vec![],
        reasoning_messages: vec![],
        turn_events: vec![],
        failure: None,
        upstream_model: None,
        reasoning_effort: None,
        request_path: None,
        speed_mode: None,
        ttft_ms: None,
        generation_speed_tps: None,
        injected_context_messages: Vec::new(),
    }
}

#[test]
fn rollout_sqlite_roundtrips_selected_skill_name() {
    let path = temp_db("selected-skill");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut session = record("skill-session", None, 100);
    session.selected_skill_name = Some("release-notes".to_owned());
    store.record_session(&session).unwrap();

    let loaded = store.load_by_id("skill-session").unwrap().unwrap();
    assert_eq!(loaded.selected_skill_name.as_deref(), Some("release-notes"));
    cleanup(&path);
}

#[test]
fn rollout_sqlite_rebuilds_incompatible_schema_to_current_columns() {
    let path = temp_db("schema-rebuild");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch(
            r"
            CREATE TABLE rollout_sessions (
                session_id TEXT PRIMARY KEY NOT NULL,
                parent_session_id TEXT,
                created_at_ms INTEGER NOT NULL,
                cwd TEXT,
                provider TEXT NOT NULL,
                model TEXT NOT NULL,
                selected_skill_name TEXT,
                prompt TEXT NOT NULL,
                response TEXT NOT NULL,
                compacted_history_json TEXT,
                usage_json TEXT,
                tool_receipts_json TEXT NOT NULL
            );
            PRAGMA user_version = 1;
            ",
        )
        .unwrap();
    drop(connection);

    let store = RolloutSqliteStore::open(&path).unwrap();
    store
        .record_session(&record("rebuilt-session", None, 100))
        .unwrap();

    let loaded = store.load_by_id("rebuilt-session").unwrap().unwrap();
    assert_eq!(
        loaded.assistant_part_id,
        agent_run_assistant_part_id(FIXTURE_ASSISTANT_TURN_ID)
    );
    cleanup(&path);
}

#[test]
fn record_and_load_by_id_roundtrips() {
    let path = temp_db("roundtrip");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut session = record("s1", None, 100);
    session.reasoning_messages = vec![super::super::run_session_store::RunReasoningMessageRecord {
        part_id: agent_run_reasoning_part_id(FIXTURE_ASSISTANT_TURN_ID),
        reasoning_id: "provider-reasoning:1".to_owned(),
        content: "推理内容".to_owned(),
    }];
    store.record_session(&session).unwrap();

    let loaded = store.load_by_id("s1").unwrap().unwrap();
    assert_eq!(loaded.session_id, "s1");
    assert_eq!(loaded.prompt, "prompt-s1");
    assert_eq!(loaded.response, "response-s1");
    assert_eq!(
        loaded.assistant_part_id,
        agent_run_assistant_part_id(FIXTURE_ASSISTANT_TURN_ID)
    );
    assert_eq!(loaded.provider, "anthropic");
    assert_eq!(loaded.reasoning_messages, session.reasoning_messages);
    cleanup(&path);
}

#[test]
fn current_session_ignores_newer_subagent_until_explicitly_selected() {
    let path = temp_db("current-session");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&record("root", None, 100)).unwrap();
    store
        .record_session(&record("root::subagent", Some("root"), 200))
        .unwrap();

    assert_eq!(store.load_latest().unwrap().unwrap().session_id, "root");

    store.select_session("root::subagent").unwrap();
    assert_eq!(
        store.load_latest().unwrap().unwrap().session_id,
        "root::subagent"
    );
    cleanup(&path);
}

#[test]
fn current_session_falls_back_to_latest_root_for_legacy_database() {
    let path = temp_db("legacy-current-session");
    let store = RolloutSqliteStore::open(&path).unwrap();
    store
        .record_session(&record("older-root", None, 100))
        .unwrap();
    store
        .record_session(&record("newer-root", None, 200))
        .unwrap();
    store
        .connection
        .execute("DELETE FROM rollout_current_session", [])
        .unwrap();
    store
        .record_session(&record("newest-child", Some("newer-root"), 300))
        .unwrap();

    assert_eq!(
        store.load_latest().unwrap().unwrap().session_id,
        "newer-root"
    );
    cleanup(&path);
}

#[test]
fn selecting_unknown_current_session_is_rejected() {
    let path = temp_db("missing-current-session");
    let store = RolloutSqliteStore::open(&path).unwrap();

    let error = store.select_session("missing").unwrap_err();

    assert!(error.contains("未找到会话"));
    cleanup(&path);
}

#[test]
fn same_session_preserves_multiple_turns_and_projects_contiguous_event_sequence() {
    let path = temp_db("multiple-turns");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut first = record("s1", None, 100);
    first.assistant_part_id = "assistant:s1:100".to_owned();
    let mut second = record("s1", None, 200);
    second.assistant_part_id = "assistant:s1:200".to_owned();
    second.prompt = "second prompt".to_owned();
    second.response = "second response".to_owned();

    store.record_session(&first).unwrap();
    store.record_session(&second).unwrap();

    let turns = store.load_history_window("s1").unwrap().turns;
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].1.prompt, "prompt-s1");
    assert_eq!(turns[1].1.prompt, "second prompt");
    let events = store.load_events("s1").unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0].sequence, 0);
    assert_eq!(events[3].sequence, 3);
    cleanup(&path);
}

#[test]
fn load_turn_by_id_returns_referenced_turn_not_latest_turn() {
    let path = temp_db("turn-id");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut first = record("s1", None, 100);
    first.assistant_part_id = "assistant:s1:100".to_owned();
    first.response = "first response".to_owned();
    let mut second = record("s1", None, 200);
    second.assistant_part_id = "assistant:s1:200".to_owned();
    second.response = "second response".to_owned();
    store.record_session(&first).unwrap();
    store.record_session(&second).unwrap();

    let loaded = store
        .load_turn_by_id("s1", "assistant:s1:100")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.response, "first response");
    assert!(
        store
            .load_turn_by_id("s1", "assistant:missing")
            .unwrap()
            .is_none()
    );
    cleanup(&path);
}

#[test]
fn rollout_sqlite_roundtrips_tool_receipt_archive_ref() {
    let path = temp_db("tool-receipt-archive-ref");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut session = record("archive-session", None, 100);
    session.tool_receipts = vec![crate::run_commands::run_tool_loop::RunToolReceipt {
        tool_call_id: 7,
        part_id: "tool-part:archive".to_owned(),
        provider_call_id: "provider-call-archive".to_owned(),
        tool_name: "builtin.shell_command".to_owned(),
        status: "completed".to_owned(),
        summary: "输出已归档".to_owned(),
        metadata: Some(serde_json::json!({
            "archiveRef": {
                "relativePath": "archive/output.log"
            }
        })),
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: Some(axon_domain::OutputRef {
            id: axon_domain::OutputRefId(7),
            byte_len: 12,
            sha256: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned(),
            source_trace: "tool://builtin/shell_command".to_owned(),
            preview: "输出已归档".to_owned(),
            archive_ref: Some(axon_domain::OutputArchiveReference {
                relative_path: "archive/output.log".to_owned(),
            }),
        }),
        duration_ms: Some(12),
    }];
    store.record_session(&session).unwrap();

    let loaded = store.load_by_id("archive-session").unwrap().unwrap();
    let output_ref = loaded.tool_receipts[0]
        .output_ref
        .as_ref()
        .expect("receipt 应恢复 archive ref");
    assert_eq!(
        output_ref.archive_ref.as_ref().unwrap().relative_path,
        "archive/output.log"
    );
    assert_eq!(output_ref.source_trace, "tool://builtin/shell_command");
    cleanup(&path);
}

#[test]
fn partial_update_rewrites_only_current_turn_event_slots() {
    let path = temp_db("incremental-turn-events");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut first = record("s1", None, 100);
    first.assistant_part_id = "assistant:s1:100".to_owned();
    let mut second = record("s1", None, 200);
    second.assistant_part_id = "assistant:s1:200".to_owned();
    store.record_session(&first).unwrap();
    store.record_session(&second).unwrap();
    let first_event_row_id = store
        .connection
        .query_row(
            "SELECT rowid FROM rollout_events WHERE session_id = 's1' AND sequence = 0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();

    second.tool_receipts = vec![crate::run_commands::run_tool_loop::RunToolReceipt {
        tool_call_id: 1,
        part_id: "tool-part:second".to_owned(),
        provider_call_id: "provider-call-second".to_owned(),
        tool_name: "builtin.read_file".to_owned(),
        status: "completed".to_owned(),
        summary: "读取完成".to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: Some(1),
    }];
    store.record_session(&second).unwrap();

    let stable_row_id = store
        .connection
        .query_row(
            "SELECT rowid FROM rollout_events WHERE session_id = 's1' AND sequence = 0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(stable_row_id, first_event_row_id);
    let events = store.load_events("s1").unwrap();
    assert_eq!(events.len(), 5);
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    cleanup(&path);
}

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

#[test]
fn rollout_events_sqlite_replays_session_events_in_sequence() {
    let path = temp_db("events");
    let mut session = record("event-session", None, 400);
    session.response = "最终响应".to_owned();
    session.tool_receipts = vec![crate::run_commands::run_tool_loop::RunToolReceipt {
        tool_call_id: 0,
        part_id: "tool-part:0".to_owned(),
        provider_call_id: "call-search".to_owned(),
        tool_name: "builtin_search".to_owned(),
        status: "completed".to_owned(),
        summary: "search ok".to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    }];
    session.reasoning_messages = vec![super::super::run_session_store::RunReasoningMessageRecord {
        part_id: agent_run_reasoning_part_id(FIXTURE_ASSISTANT_TURN_ID),
        reasoning_id: "provider-reasoning:1".to_owned(),
        content: "检索后推理".to_owned(),
    }];
    let store = RolloutSqliteStore::open(&path).unwrap();
    store.record_session(&session).unwrap();
    let loaded = store.load_events("event-session").unwrap();

    assert_eq!(loaded.len(), 4);
    assert_eq!(loaded[0].event_type, RolloutEventType::SessionStarted);
    assert_eq!(loaded[1].event_type, RolloutEventType::ToolReceipts);
    assert_eq!(loaded[2].event_type, RolloutEventType::ReasoningMessages);
    assert_eq!(loaded[3].event_type, RolloutEventType::SessionCompleted);
    assert_eq!(
        loaded[0].payload["promptChars"],
        "prompt-event-session".chars().count()
    );
    assert_eq!(loaded[1].payload["toolReceiptCount"], 1);
    assert_eq!(
        loaded[1].payload["outputRefs"].as_array().map(Vec::len),
        Some(0)
    );
    assert_eq!(
        loaded[2].payload["reasoningMessageCount"],
        session.reasoning_messages.len()
    );
    assert_eq!(loaded[2].payload["reasoningIds"][0], "provider-reasoning:1");
    assert_eq!(
        loaded[3].payload["responseChars"],
        "最终响应".chars().count()
    );
    assert_eq!(loaded[3].payload["reasoningMessageCount"], 1);
    cleanup(&path);
}

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
    let path = super::resolved_rollout_db_path(&config_path).unwrap();
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
        super::read_rollout_tool_output(&config_path, "root", "provider-call-root").unwrap(),
        "完整归档正文".as_bytes()
    );

    let store = RolloutSqliteStore::open(&path).unwrap();
    let child = record("root::child", Some("root"), 200);
    store.record_session(&child).unwrap();
    drop(store);
    let replayed = super::replay_session_chain(&config_path, "root::child").unwrap();
    assert_eq!(replayed.len(), 2, "replay 应保留完整 resume 链");
    assert_eq!(
        super::read_rollout_tool_output(&config_path, "root", "provider-call-root").unwrap(),
        "完整归档正文".as_bytes()
    );
    assert!(
        super::read_rollout_tool_output(&config_path, "root::child", "provider-call-root").is_err()
    );

    let archive_path = archive_root.join(
        output_ref
            .archive_ref
            .as_ref()
            .expect("OutputRef 应保存相对归档路径")
            .relative_path
            .clone(),
    );
    std::fs::write(archive_path, "被篡改的归档正文").unwrap();
    assert!(super::read_rollout_tool_output(&config_path, "root", "provider-call-root").is_err());
    let _ = std::fs::remove_dir_all(archive_root);
}

fn cleanup(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir_all(parent);
    }
}
