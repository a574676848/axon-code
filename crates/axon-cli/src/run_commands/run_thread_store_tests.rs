use super::*;
use crate::run_commands::run_session_store::RunReasoningMessageRecord;
use axon_agent_runtime::{
    AgentModelMessage, AgentModelToolCallMetadata, AgentModelToolResultMetadata, ProviderToolCall,
};
use axon_session::{
    ThreadCompaction, ThreadEvent, ThreadEventStore as _, ThreadHistoryFileRef,
    ThreadHistoryImageRef, ThreadHistoryItem, ThreadHistoryStatus,
};

const TEST_FREEFORM_PATCH: &str =
    "*** Begin Patch\n*** Add File: src/session.rs\n+pub fn session() {}\n*** End Patch";

#[path = "run_thread_store_model_history_tests.rs"]
mod model_history_tests;

#[test]
fn structured_tool_history_roundtrips_complete_arguments_operation_and_receipt() {
    let record = session_record_with_tools();
    let turn = turn_from_record(&record, 0).unwrap();
    let json = serde_json::to_string(&turn).unwrap();
    let restored: Turn = serde_json::from_str(&json).unwrap();

    let ThreadHistoryItem::ToolCall {
        provider_call_id,
        operation_id,
        input,
        ..
    } = &restored.history_items[1]
    else {
        panic!("第二个 item 应为 tool-call");
    };
    assert_eq!(provider_call_id, "call-search");
    assert_eq!(input.canonical_text(), r#"{"query":"完整参数"}"#);
    assert!(operation_id.contains("tool-part:search"));

    let ThreadHistoryItem::ToolResult { receipt, .. } = &restored.history_items[2] else {
        panic!("第三个 item 应为 tool-result");
    };
    assert_eq!(
        receipt.as_ref().map(|value| value.summary.as_str()),
        Some("找到结果")
    );
}

#[test]
fn freeform_tool_input_roundtrips_session_replay_and_compaction() {
    let mut record = session_record_with_tools();
    record.tool_calls[0] = ProviderToolCall::new(
        "call-search",
        "builtin.edit",
        axon_domain::ToolCallInput::text(TEST_FREEFORM_PATCH),
    );
    record.tool_receipts[0].tool_name = "builtin.edit".to_owned();

    let turn = turn_from_record(&record, 0).unwrap();
    let ThreadHistoryItem::ToolCall { input, .. } = &turn.history_items[1] else {
        panic!("第二个 item 应为 freeform tool-call");
    };
    assert_eq!(input.as_text(), Some(TEST_FREEFORM_PATCH));

    let mut detail = thread_detail_from_records(&[record]).unwrap();
    let replayed = parsed_model_history(&detail);
    assert_eq!(
        model_tool_calls(&replayed[1])[0].input.as_text(),
        Some(TEST_FREEFORM_PATCH)
    );

    let history_json = serde_json::to_string(&replayed).unwrap();
    let through_sequence = detail.turns[0].sequence;
    detail.compacted_history = Some(ThreadCompaction {
        history_json,
        through_sequence,
        checkpoint_text: "freeform checkpoint".to_owned(),
        before_tokens: 100,
        after_tokens: 10,
        recorded_at_ms: 400,
    });
    detail.turns.clear();

    let compacted = parsed_model_history(&detail);
    assert_eq!(
        model_tool_calls(&compacted[1])[0].input.as_text(),
        Some(TEST_FREEFORM_PATCH)
    );
}

#[test]
fn compaction_appends_only_newer_structured_items() {
    let records = vec![
        session_record("first-user", "first-assistant", 100),
        session_record_with_tools(),
    ];
    let mut detail = thread_detail_from_records(&records).unwrap();
    detail.compacted_history = Some(ThreadCompaction {
        history_json: serde_json::to_string(&[AgentModelMessage::cache_excluded(
            "user",
            "checkpoint",
        )])
        .unwrap(),
        through_sequence: 0,
        checkpoint_text: "checkpoint".to_owned(),
        before_tokens: 100,
        after_tokens: 10,
        recorded_at_ms: 200,
    });

    let messages = parsed_model_history(&detail);
    let calls = model_tool_calls(&messages[2]);
    let result = model_tool_result(&messages[3]).expect("压缩点后的 result 必须保留");

    assert_eq!(messages.len(), 5);
    assert_eq!(messages[0].content, "checkpoint");
    assert_eq!(calls[0].id, "call-search");
    assert_eq!(result.call_id, "call-search");
}

#[test]
fn checkpoint_window_resume_does_not_reinject_raw_prefix() {
    let fallback = session_record("third-user", "third-assistant", 300);
    let detail = thread_detail_from_history_window(
        &fallback,
        RolloutHistoryWindow {
            compaction: Some(ThreadCompaction {
                history_json: serde_json::json!([
                    {"role": "user", "content": "checkpoint", "cache_excluded": true}
                ])
                .to_string(),
                through_sequence: 1,
                checkpoint_text: "checkpoint".to_owned(),
                before_tokens: 100,
                after_tokens: 10,
                recorded_at_ms: 200,
            }),
            turns: vec![(2, fallback.clone())],
        },
    )
    .unwrap();

    let history = thread_model_history_json(&detail).unwrap();
    let messages: Vec<serde_json::Value> = serde_json::from_str(&history).unwrap();

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["content"], "checkpoint");
    assert_eq!(messages[1]["content"], "third-user");
    assert_eq!(messages[2]["content"], "third-assistant");
    assert!(!history.contains("first-user"));
    assert!(!history.contains("second-user"));
}

#[test]
fn repeated_provider_call_id_pairs_by_stable_part_and_operation() {
    let mut record = session_record_with_tools();
    record.tool_calls[0].id = "shared-call".to_owned();
    record.tool_receipts[0].provider_call_id = "shared-call".to_owned();
    record.tool_receipts[0].part_id = "tool-part:stream:1:0:shared-call".to_owned();
    record.tool_calls.push(ProviderToolCall::new(
        "shared-call",
        "builtin.read_file",
        r#"{"path":"Cargo.toml"}"#,
    ));
    let mut second = record.tool_receipts[0].clone();
    second.tool_call_id = 8;
    second.part_id = "tool-part:stream:2:0:shared-call".to_owned();
    second.tool_name = "builtin.read_file".to_owned();
    record.tool_receipts.push(second);
    record.tool_receipts.swap(0, 1);
    record.turn_events = vec![
        tool_started_event_for_turn(1, "tool-part:stream:1:0:shared-call", "shared-call"),
        tool_started_event_for_turn(2, "tool-part:stream:2:0:shared-call", "shared-call"),
    ];

    let turn = turn_from_record(&record, 0).unwrap();
    let pairs = turn
        .history_items
        .iter()
        .filter_map(|item| match item {
            ThreadHistoryItem::ToolCall {
                part_id,
                tool_call_id,
                operation_id,
                ..
            } => Some((
                part_id.as_str(),
                tool_call_id.as_str(),
                operation_id.as_str(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(pairs[0].0, "tool-part:stream:1:0:shared-call");
    assert_eq!(pairs[0].1, "7");
    assert_eq!(pairs[1].0, "tool-part:stream:2:0:shared-call");
    assert_eq!(pairs[1].1, "8");
    assert_ne!(pairs[0].2, pairs[1].2);

    let detail = thread_detail_from_records(&[record]).unwrap();
    let messages = parsed_model_history(&detail);
    let tool_batches = messages
        .iter()
        .filter(|message| !model_tool_calls(message).is_empty())
        .collect::<Vec<_>>();
    assert_eq!(tool_batches.len(), 2);
    assert_eq!(model_tool_calls(tool_batches[0])[0].id, "shared-call");
    assert_eq!(model_tool_calls(tool_batches[1])[0].id, "shared-call");
}

#[test]
fn parallel_tool_calls_restore_one_batch_and_pair_swapped_receipts_by_part() {
    let mut record = session_record_with_tools();
    record.tool_calls.push(ProviderToolCall::new(
        "call-read",
        "builtin.read_file",
        r#"{"path":"Cargo.toml"}"#,
    ));
    let mut second = record.tool_receipts[0].clone();
    second.tool_call_id = 8;
    second.part_id = "tool-part:read".to_owned();
    second.provider_call_id = "call-read".to_owned();
    second.tool_name = "builtin.read_file".to_owned();
    second.summary = "读取完成".to_owned();
    record.tool_receipts.push(second);
    record.tool_receipts.swap(0, 1);
    record.turn_events = vec![
        tool_started_event_for_turn(7, "tool-part:search", "call-search"),
        tool_started_event_for_turn(7, "tool-part:read", "call-read"),
    ];

    let detail = thread_detail_from_records(&[record]).unwrap();
    let messages = parsed_model_history(&detail);
    let calls = model_tool_calls(&messages[1]);
    let search_result = model_tool_result(&messages[2]).expect("search result 必须保留");
    let read_result = model_tool_result(&messages[3]).expect("read result 必须保留");

    assert_eq!(messages.len(), 5);
    assert_eq!(calls.len(), 2);
    assert_eq!(search_result.call_id, "call-search");
    assert!(search_result.output.contains("找到结果"));
    assert_eq!(read_result.call_id, "call-read");
    assert!(read_result.output.contains("读取完成"));
}

#[test]
fn partial_updates_append_only_new_history_items() {
    let root = temp_root("linear-events");
    let service = ThreadService::new(JsonlThreadEventStore);
    let mut first = session_record("用户问题", "", 300);
    first.tool_calls = vec![ProviderToolCall::new(
        "call-first",
        "builtin.shell_command",
        r#"{"command":"cargo check"}"#,
    )];
    first.turn_events = vec![tool_started_event(
        "tool-part:stream:1:0:call-first",
        "call-first",
    )];
    let first_turn = turn_from_record(&first, 0).unwrap();
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: first.session_id.clone(),
                session_root_id: first.session_id.clone(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(&first),
                created_at_ms: first.created_at_ms,
            },
        )
        .unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: first.session_id.clone(),
                recorded_at_ms: first.created_at_ms,
                turn: first_turn,
            },
        )
        .unwrap();

    let mut second = first.clone();
    second.created_at_ms = 301;
    second.tool_calls.push(ProviderToolCall::new(
        "call-second",
        "builtin.read_file",
        r#"{"path":"Cargo.toml"}"#,
    ));
    second.turn_events.push(tool_started_event(
        "tool-part:stream:1:1:call-second",
        "call-second",
    ));
    let detail = service.read_thread(&root, &second.session_id).unwrap();
    mirror_existing_turn(
        &service,
        &root,
        &second.session_id,
        &detail,
        turn_from_record(&second, 0).unwrap(),
        &second,
    )
    .unwrap();

    let events = JsonlThreadEventStore.read_all(&root).unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0].event, ThreadEvent::ThreadStarted { .. }));
    assert!(matches!(events[1].event, ThreadEvent::TurnStarted { .. }));
    let ThreadEvent::TurnItemsAppended { items, .. } = &events[2].event else {
        panic!("第二次 partial 应只追加新增历史 item");
    };
    assert_eq!(items.len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn partial_tool_call_restores_active_turn_and_execution_permission_metadata() {
    let mut record = session_record("用户问题", "", 300);
    record.tool_calls = vec![ProviderToolCall::new(
        "call-pending",
        "builtin.shell_command",
        r#"{"command":"cargo test"}"#,
    )];
    record.turn_events = vec![
        tool_started_event("tool:call-pending", "call-pending")
            .with_tool_name("builtin.shell_command")
            .with_status("running")
            .with_phase("execution")
            .with_metadata(serde_json::json!({
                "executionPermission": "allowed",
            })),
    ];

    let turn = turn_from_record(&record, 0).unwrap();
    assert_eq!(turn.status, axon_session::TurnStatus::Active);
    assert!(turn.completed_at_ms.is_none());
    let ThreadHistoryItem::ToolCall {
        status,
        provider_metadata,
        ..
    } = &turn.history_items[1]
    else {
        panic!("第二个 item 应为未完成 tool-call");
    };
    assert_eq!(*status, ThreadHistoryStatus::InProgress);
    assert_eq!(
        provider_metadata
            .as_ref()
            .and_then(|value| value.get("executionPermission"))
            .and_then(serde_json::Value::as_str),
        Some("allowed")
    );
}

#[test]
fn structured_history_omits_tool_items_without_provider_turn_event() {
    let mut record = session_record_with_tools();
    record.turn_events.clear();

    let turn = turn_from_record(&record, 0).unwrap();

    assert!(turn.history_items.iter().all(|item| !matches!(
        item,
        ThreadHistoryItem::ToolCall { .. } | ThreadHistoryItem::ToolResult { .. }
    )));
}

#[test]
fn structured_history_omits_reasoning_with_unparseable_provider_turn_id() {
    let mut record = session_record("用户问题", "最终回答", 300);
    record.reasoning_messages = vec![
        RunReasoningMessageRecord {
            part_id: "reasoning-invalid-number".to_owned(),
            reasoning_id: "provider-reasoning:not-a-number".to_owned(),
            content: "非数字 reasoning".to_owned(),
        },
        RunReasoningMessageRecord {
            part_id: "reasoning-invalid-prefix".to_owned(),
            reasoning_id: "legacy-reasoning:3".to_owned(),
            content: "非 canonical reasoning".to_owned(),
        },
        RunReasoningMessageRecord {
            part_id: "reasoning-invalid-zero".to_owned(),
            reasoning_id: "provider-reasoning:0".to_owned(),
            content: "保留 turn reasoning".to_owned(),
        },
        RunReasoningMessageRecord {
            part_id: "reasoning-valid".to_owned(),
            reasoning_id: "provider-reasoning:2".to_owned(),
            content: "有效 reasoning".to_owned(),
        },
    ];

    let turn = turn_from_record(&record, 0).unwrap();
    let reasoning = turn
        .history_items
        .iter()
        .filter_map(|item| match item {
            ThreadHistoryItem::Reasoning { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(reasoning, vec!["有效 reasoning"]);
}

#[test]
fn run_session_record_without_local_images_is_rejected() {
    assert_missing_attachment_field_is_rejected("localImages");
}

#[test]
fn run_session_record_without_local_files_is_rejected() {
    assert_missing_attachment_field_is_rejected("localFiles");
}

fn assert_missing_attachment_field_is_rejected(field: &str) {
    let mut value = serde_json::to_value(session_record("分析附件", "已完成", 300)).unwrap();
    let removed = value.as_object_mut().unwrap().remove(field);
    assert!(removed.is_some(), "测试记录必须包含 {field}");

    let error = serde_json::from_value::<RunSessionRecord>(value).unwrap_err();
    assert!(error.to_string().contains(field));
}

#[test]
fn attachments_roundtrip_sqlite_thread_jsonl_and_resume_history() {
    let root = temp_root("attachment-resume");
    std::fs::create_dir_all(&root).unwrap();
    let config_path = root.join("isolated-config.toml");
    std::fs::write(&config_path, "[agent]\ndefault_model = \"balanced\"\n").unwrap();
    let mut record = session_record("分析附件", "已完成", 300);
    record.session_id = "attachment-session".to_owned();
    record.local_images = vec![ThreadHistoryImageRef {
        path: String::new(),
        mime_type: "image/png".to_owned(),
        byte_len: Some(8),
        sha256: Some("image-sha".to_owned()),
        archive_ref: Some("blobs/image-sha.blob".to_owned()),
        source_ref: Some("local-upload://image.png".to_owned()),
    }];
    record.local_files = vec![ThreadHistoryFileRef {
        name: "notes.txt".to_owned(),
        path: Some("C:\\work\\notes.txt".to_owned()),
        mime_type: "text/plain".to_owned(),
        preview: "文件预览".to_owned(),
        byte_len: Some(12),
        sha256: Some("file-sha".to_owned()),
        archive_ref: Some("blobs/file-sha.blob".to_owned()),
        source_ref: Some("local-upload://notes.txt".to_owned()),
    }];

    let db_path = super::super::run_rollout_store::resolved_rollout_db_path(&config_path).unwrap();
    super::super::run_rollout_store::RolloutSqliteStore::open(&db_path)
        .unwrap()
        .record_session(&record)
        .unwrap();
    mirror_session_to_thread(&config_path, &record).unwrap();

    let stored_thread = ThreadService::new(JsonlThreadEventStore)
        .read_thread(&root, &record.session_id)
        .unwrap();
    let ThreadHistoryItem::UserMessage {
        local_images,
        local_files,
        ..
    } = &stored_thread.turns[0].history_items[0]
    else {
        panic!("首个 Thread item 应为用户消息");
    };
    assert_eq!(local_images, &record.local_images);
    assert_eq!(local_files, &record.local_files);

    let prepared = prepare_run_session(
        &config_path,
        &RunCommand {
            prompt: "继续".to_owned(),
            model: None,
            reasoning_effort: None,
            format: None,
            continue_last: false,
            session: Some(record.session_id.clone()),
            file: Vec::new(),
            timeout: None,
            tool: Vec::new(),
        },
        None,
    )
    .unwrap()
    .expect("应恢复持久化 session");
    assert_eq!(prepared.record.local_images, record.local_images);
    assert_eq!(prepared.record.local_files, record.local_files);
    let messages: Vec<AgentModelMessage> =
        serde_json::from_str(&prepared.model_history_json).unwrap();
    assert_eq!(
        messages[0].local_images[0].archive_ref.as_deref(),
        Some("blobs/image-sha.blob")
    );
    assert_eq!(
        messages[0].local_files[0].archive_ref.as_deref(),
        Some("blobs/file-sha.blob")
    );
    assert_eq!(messages[0].local_files[0].name, "notes.txt");

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_active_turn_is_recovered_before_mirroring_next_run() {
    let root = temp_root("stale-active-recovery");
    std::fs::create_dir_all(&root).unwrap();
    let config_path = root.join("config.toml");
    let service = ThreadService::new(JsonlThreadEventStore);

    // 构造“上一进程被强杀”的遗留现场：active turn 没有 terminal 事件。
    let mut crashed = session_record("上一轮未收口", "", 400);
    crashed.tool_calls = vec![ProviderToolCall::new(
        "call-pending",
        "builtin.shell_command",
        r#"{"command":"cargo test"}"#,
    )];
    crashed.turn_events =
        vec![tool_started_event("tool:call-pending", "call-pending").with_status("running")];
    let stale_turn = turn_from_record(&crashed, 0).unwrap();
    assert_eq!(stale_turn.status, axon_session::TurnStatus::Active);
    service
        .start_empty_thread(
            &root,
            StartEmptyThreadCommand {
                thread_id: crashed.session_id.clone(),
                session_root_id: crashed.session_id.clone(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: settings_from_record(&crashed),
                created_at_ms: crashed.created_at_ms,
            },
        )
        .unwrap();
    service
        .start_turn(
            &root,
            StartTurnCommand {
                thread_id: crashed.session_id.clone(),
                recorded_at_ms: crashed.created_at_ms,
                turn: stale_turn.clone(),
            },
        )
        .unwrap();

    // 新进程镜像同一会话的新 turn：遗留 active turn 必须先被中断，否则整轮会被永久拒绝。
    let mut fresh = session_record("现在几点", "12:00", 500);
    fresh.assistant_part_id = "assistant:session-1:fresh".to_owned();
    mirror_session_started(&config_path, &fresh).unwrap();

    let detail = service.read_thread(&root, &fresh.session_id).unwrap();
    let recovered = detail
        .turns
        .iter()
        .find(|turn| turn.turn_id == stale_turn.turn_id)
        .expect("遗留 turn 必须保留在历史中");
    assert_eq!(recovered.status, axon_session::TurnStatus::Interrupted);
    assert!(
        detail
            .turns
            .iter()
            .any(|turn| turn.status == axon_session::TurnStatus::Active
                && turn.turn_id != stale_turn.turn_id),
        "新 turn 必须已开始"
    );
    let _ = std::fs::remove_dir_all(root);
}

fn session_record_with_tools() -> RunSessionRecord {
    let mut record = session_record("用户问题", "最终回答", 300);
    record.tool_calls = vec![ProviderToolCall::new(
        "call-search",
        "mcp.search",
        r#"{"query":"完整参数"}"#,
    )];
    record.tool_receipts = vec![super::super::run_execution::RunToolReceipt {
        tool_call_id: 7,
        part_id: "tool-part:search".to_owned(),
        provider_call_id: "call-search".to_owned(),
        tool_name: "mcp.search".to_owned(),
        status: "completed".to_owned(),
        summary: "找到结果".to_owned(),
        metadata: Some(serde_json::json!({"source": "mcp"})),
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: Some(12),
    }];
    record.turn_events = vec![tool_started_event_for_turn(
        1,
        "tool-part:search",
        "call-search",
    )];
    record
}

fn session_record(user: &str, assistant: &str, at_ms: u64) -> RunSessionRecord {
    RunSessionRecord {
        session_id: "session-1".to_owned(),
        parent_session_id: None,
        created_at_ms: at_ms,
        startup_timeline: crate::run_commands::run_startup_snapshot::test_startup_timeline(),
        cwd: Some("E:/zbg/axon-code".to_owned()),
        provider: "provider-a".to_owned(),
        model: "model-a".to_owned(),
        selected_skill_name: None,
        repo_context: None,
        session_context: None,
        durable_memory: None,
        prompt: user.to_owned(),
        local_images: Vec::new(),
        local_files: Vec::new(),
        response: assistant.to_owned(),
        assistant_part_id: "assistant:session-1:stable".to_owned(),
        usage: None,
        tool_receipts: Vec::new(),
        tool_calls: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
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

fn tool_started_event(part_id: &str, provider_call_id: &str) -> axon_agent_runtime::AgentTurnEvent {
    tool_started_event_for_turn(1, part_id, provider_call_id)
}

fn tool_started_event_for_turn(
    turn_id: u64,
    part_id: &str,
    provider_call_id: &str,
) -> axon_agent_runtime::AgentTurnEvent {
    axon_agent_runtime::AgentTurnEvent::new(
        axon_domain::TurnId(turn_id),
        axon_agent_runtime::AgentTurnEventKind::ToolExecutionStarted,
        "工具开始",
    )
    .with_part_id(part_id)
    .with_provider_call_id(provider_call_id)
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-thread-store-{tag}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

fn parsed_model_history(detail: &ThreadDetail) -> Vec<AgentModelMessage> {
    let history = thread_model_history_json(detail).expect("模型历史必须可序列化");
    serde_json::from_str(&history).expect("模型历史必须符合 AgentModelMessage 合同")
}

fn model_tool_calls(message: &AgentModelMessage) -> &[AgentModelToolCallMetadata] {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_calls.as_deref())
        .unwrap_or_default()
}

fn model_tool_result(message: &AgentModelMessage) -> Option<&AgentModelToolResultMetadata> {
    message
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
}
