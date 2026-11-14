//! rollout turn 写入、读取、当前会话选择与事件投影测试。

use super::super::{RolloutEventType, RolloutSqliteStore};
use super::{FIXTURE_ASSISTANT_TURN_ID, cleanup, record, temp_db};
use axon_agent_runtime::{agent_run_assistant_part_id, agent_run_reasoning_part_id};

#[test]
fn latest_reflection_notice_reads_only_the_newest_turn() {
    let path = temp_db("latest-reflection-notice");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let mut older = record("notice-session", None, 100);
    older.reflection_notice = Some("第一轮的提示".to_owned());
    store.record_session(&older).unwrap();
    let mut newer = record("notice-session", None, 200);
    newer.assistant_part_id = "assistant-run:notice-2".to_owned();
    newer.reflection_notice = Some("第二轮的提示".to_owned());
    store.record_session(&newer).unwrap();

    assert_eq!(
        store
            .latest_reflection_notice("notice-session")
            .unwrap()
            .as_deref(),
        Some("第二轮的提示"),
        "只读接口要最新一轮，不是最早那一轮"
    );
    cleanup(&path);
}

#[test]
fn latest_reflection_notice_is_none_when_the_turn_has_no_notice() {
    let path = temp_db("no-reflection-notice");
    let store = RolloutSqliteStore::open(&path).unwrap();
    let session = record("quiet-session", None, 100);
    store.record_session(&session).unwrap();

    assert_eq!(
        store.latest_reflection_notice("quiet-session").unwrap(),
        None,
        "该轮没有提示时必须读作空，不能编造一条"
    );
    cleanup(&path);
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
    session.reasoning_messages = vec![
        crate::run_commands::run_session_store::RunReasoningMessageRecord {
            part_id: agent_run_reasoning_part_id(FIXTURE_ASSISTANT_TURN_ID),
            reasoning_id: "provider-reasoning:1".to_owned(),
            content: "推理内容".to_owned(),
        },
    ];
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
    session.reasoning_messages = vec![
        crate::run_commands::run_session_store::RunReasoningMessageRecord {
            part_id: agent_run_reasoning_part_id(FIXTURE_ASSISTANT_TURN_ID),
            reasoning_id: "provider-reasoning:1".to_owned(),
            content: "检索后推理".to_owned(),
        },
    ];
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
