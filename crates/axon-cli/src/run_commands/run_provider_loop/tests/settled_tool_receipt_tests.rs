use super::*;
use crate::run_commands::run_session_store::{RunSessionIndexLookup, load_indexed_run_session};
use axon_agent_runtime::{
    AgentRunToolOutputPolicy, ContentArchiveRuntime, agent_run_tool_result_messages,
    agent_run_tool_result_receipt_from_facts,
};

/// 超过固定内联预算的 metadata 条目数：只有越过预算的用例才该走归档。
const OVERSIZED_METADATA_ENTRIES: usize = 4_000;
/// 超过固定内联预算的正文规模。
const OVERSIZED_SUMMARY_REPEAT: usize = 20_000;
/// 仅比固定内联预算大一点的正文规模，用于形状非法的 metadata 用例。
const OVERSIZED_PLAIN_REPEAT: usize = 60_000;

#[test]
fn settled_tool_receipt_is_durable_before_next_provider_turn() {
    let root = temp_workspace("settled-tool-receipt");
    let request = fixture_request(root.clone());
    let tool_call = axon_agent_runtime::ProviderToolCall::new(
        "provider-call-settled",
        "builtin.edit",
        r#"{"patch":"*** Begin Patch\\n*** End Patch"}"#,
    );
    let part_id = crate::tool_orchestrator::tool_execution_part_id(1, &tool_call.id, 0);
    let receipts = RefCell::new(Vec::new());
    let calls = RefCell::new(vec![tool_call]);
    let reasoning = RefCell::new(Vec::new());
    let events = RefCell::new(vec![
        partial_tool_call_events(1, calls.borrow().as_slice()).remove(0),
    ]);
    let state = PartialToolCallState {
        tool_receipts: &receipts,
        tool_calls: &calls,
        reasoning: &reasoning,
        turn_events: &events,
        provider_name: "fixture",
        model_id: "fixture-model",
    };
    let receipt = RunToolReceipt {
        tool_call_id: 7,
        part_id: part_id.clone(),
        provider_call_id: "provider-call-settled".to_owned(),
        tool_name: "builtin.edit".to_owned(),
        status: "completed".to_owned(),
        summary: "patch applied".to_owned(),
        metadata: Some(serde_json::json!({"kind": "edit"})),
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: true,
        output_ref: None,
        duration_ms: Some(11),
    };

    persist_settled_tool_receipts(&request, &state, 1, &[receipt]);

    let persisted = load_indexed_run_session(
        &request.config_path,
        &RunSessionIndexLookup::SessionId(&request.session_id),
    )
    .expect("rollout SSOT 应可读取")
    .expect("部分工具回执应在下一轮 Provider 前可读");
    assert_eq!(
        persisted.assistant_part_id,
        crate::run_commands::run_session_store::run_persistence_assistant_part_id(&request)
    );
    assert!(persisted.tool_receipts.iter().any(|value| {
        value.part_id == part_id
            && value.status == "completed"
            && value.filesystem_write_performed
            && value.duration_ms == Some(11)
    }));

    persist_partial_tool_call_record(&request, &state, 2);
    let rollout_path = crate::run_commands::resolved_rollout_db_path(&request.config_path).unwrap();
    let turn_count = rusqlite::Connection::open(rollout_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM rollout_turns WHERE session_id = ?1",
            rusqlite::params![request.session_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(turn_count, 1);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_metadata_receipt_is_archived_under_data_root_and_replayable() {
    let root = temp_workspace("oversized-metadata-receipt");
    let request = fixture_request(root.clone());
    let tool_call = axon_agent_runtime::ProviderToolCall::new(
        "provider-call-metadata",
        "builtin.search",
        r#"{"query":"archive"}"#,
    );
    let receipts = RefCell::new(Vec::new());
    let calls = RefCell::new(vec![tool_call]);
    let reasoning = RefCell::new(Vec::new());
    let events = RefCell::new(partial_tool_call_events(1, calls.borrow().as_slice()));
    let state = PartialToolCallState {
        tool_receipts: &receipts,
        tool_calls: &calls,
        reasoning: &reasoning,
        turn_events: &events,
        provider_name: "fixture",
        model_id: "fixture-model",
    };
    let mut receipt = completed_receipt(
        "provider-call-metadata",
        "search entries=300",
        Some(serde_json::json!({
            "entries": (0..OVERSIZED_METADATA_ENTRIES)
                .map(|index| serde_json::json!({"name": format!("entry-{index:03}")}))
                .collect::<Vec<_>>(),
        })),
    );

    archive_oversized_tool_receipts(&request, std::slice::from_mut(&mut receipt));
    let output_ref = receipt.output_ref.clone().expect("超预算 metadata 应归档");
    assert_eq!(
        receipt.metadata.as_ref().unwrap()["archiveRef"]["id"],
        output_ref.id.0
    );
    assert!(
        receipt.metadata.as_ref().unwrap()["archivePath"]
            .as_str()
            .is_some_and(|path| std::path::Path::new(path).is_absolute())
    );
    let relative_path = &output_ref.archive_ref.as_ref().unwrap().relative_path;
    assert!(std::path::Path::new(relative_path).starts_with("archive"));
    assert!(!root.join("archive").exists());

    persist_settled_tool_receipts(&request, &state, 1, &[receipt]);
    let archived = crate::run_commands::read_rollout_tool_output(
        &request.config_path,
        &request.session_id,
        "provider-call-metadata",
    )
    .expect("显式 provider call id 应读取完整归档");
    let payload: serde_json::Value = serde_json::from_slice(&archived).unwrap();
    assert_eq!(payload["summary"], "search entries=300");
    assert_eq!(
        payload["metadata"]["entries"].as_array().unwrap().len(),
        OVERSIZED_METADATA_ENTRIES
    );

    let _ = fs::remove_dir_all(root);
}

#[test]
fn oversized_summary_archive_survives_low_budget_provider_projection() {
    let root = temp_workspace("oversized-summary-receipt");
    let request = fixture_request(root.clone());
    let summary = format!(
        "summary-head\n{}\nsummary-tail",
        "正文".repeat(OVERSIZED_SUMMARY_REPEAT)
    );
    let mut receipt = completed_receipt("provider-call-summary", &summary, None);

    archive_oversized_tool_receipts(&request, std::slice::from_mut(&mut receipt));
    let runtime_receipt = agent_run_tool_result_receipt_from_facts(&receipt);
    let messages = agent_run_tool_result_messages(
        &[axon_agent_runtime::ProviderToolCall::new(
            "provider-call-summary",
            "builtin.search",
            r#"{"query":"summary"}"#,
        )],
        &[runtime_receipt],
        None,
        AgentRunToolOutputPolicy::inline_budget(),
    )
    .expect("带归档引用的长摘要应可投影");
    let tool_result = messages[1]
        .provider_metadata
        .as_ref()
        .and_then(|metadata| metadata.tool_result.as_ref())
        .expect("应生成 tool result");
    let provider_payload: serde_json::Value = serde_json::from_str(&tool_result.output).unwrap();

    assert!(
        tool_result.output.len() <= AgentRunToolOutputPolicy::inline_budget().max_output_bytes()
    );
    assert!(provider_payload["output_ref"]["archiveRef"]["relativePath"].is_string());
    assert!(
        provider_payload["output_ref"]["readPath"]
            .as_str()
            .is_some_and(|path| std::path::Path::new(path).is_absolute())
    );
    assert!(provider_payload["archive_read_hint"].as_str().is_some());
    let archived = read_archived_payload(&request, &receipt);
    assert_eq!(archived["summary"], summary);

    let _ = fs::remove_dir_all(root);
}

#[test]
fn existing_output_ref_is_not_archived_again() {
    let root = temp_workspace("existing-output-ref");
    let request = fixture_request(root.clone());
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(&request.config_path)
        .expect("data 根应可解析");
    let existing_ref = ContentArchiveRuntime::new(1)
        .archive_text(
            &data_root,
            axon_agent_runtime::CONTENT_ARCHIVE_RESULT_STREAM,
            "existing output",
            "builtin/search",
        )
        .unwrap()
        .output_ref
        .unwrap();
    let mut receipt = completed_receipt("provider-call-existing", &"x".repeat(2_000), None);
    receipt.output_ref = Some(existing_ref.clone());

    archive_oversized_tool_receipts(&request, std::slice::from_mut(&mut receipt));

    assert_eq!(receipt.output_ref.as_ref(), Some(&existing_ref));
    assert!(receipt.metadata.is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn invalid_metadata_shape_blocks_oversized_receipt() {
    let root = temp_workspace("invalid-metadata-shape");
    let request = fixture_request(root.clone());
    let mut receipt = completed_receipt(
        "provider-call-invalid-metadata",
        &"x".repeat(OVERSIZED_PLAIN_REPEAT),
        Some(serde_json::json!(["not-an-object"])),
    );

    archive_oversized_tool_receipts(&request, std::slice::from_mut(&mut receipt));

    assert_eq!(receipt.status, "blocked");
    assert_eq!(receipt.error_type.as_deref(), Some("blocked"));
    assert_eq!(receipt.metadata.as_ref().unwrap()["archiveFailed"], true);
    assert!(receipt.output_ref.is_none());
    let _ = fs::remove_dir_all(root);
}

fn completed_receipt(
    provider_call_id: &str,
    summary: &str,
    metadata: Option<serde_json::Value>,
) -> RunToolReceipt {
    RunToolReceipt {
        tool_call_id: 7,
        part_id: format!("tool-part:{provider_call_id}"),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: "builtin.search".to_owned(),
        status: "completed".to_owned(),
        summary: summary.to_owned(),
        metadata,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: Some(11),
    }
}

fn read_archived_payload(request: &RunRequest, receipt: &RunToolReceipt) -> serde_json::Value {
    let data_root = crate::runtime_paths::runtime_axon_data_path_static(&request.config_path)
        .expect("data 根应可解析");
    let output_ref = receipt.output_ref.as_ref().expect("应包含 OutputRef");
    let bytes = ContentArchiveRuntime::default()
        .read_output(&data_root, output_ref)
        .expect("完整归档应可读取");
    serde_json::from_slice(&bytes).expect("完整归档应为合法 JSON")
}
