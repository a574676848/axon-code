use super::{
    SELF_ITERATION_RECEIPT_STORE_SCHEMA_VERSION, SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE,
    SelfIterationHookKind, SelfIterationReceiptInput, SelfIterationReceiptRecord,
    SelfIterationReceiptStoreRuntime, SelfIterationSubagentStatus, SelfIterationWriteDecision,
};
use axon_jsonl::ShardedJsonlStore;

#[test]
fn self_iteration_receipt_store_persists_structured_receipts_without_raw_transcript() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-self-iteration-receipts-test-{}",
        std::process::id()
    ));
    let path = SelfIterationReceiptStoreRuntime::resolve_store_path(&root);
    let memory = SelfIterationReceiptRecord::from_input(receipt_input(
        SelfIterationHookKind::MemoryUpdate,
        1_000,
        SelfIterationSubagentStatus::CompletedWithWrite,
        SelfIterationWriteDecision::Persisted,
    ))
    .unwrap();
    let reflection = SelfIterationReceiptRecord::from_input(receipt_input(
        SelfIterationHookKind::SkillReflection,
        2_000,
        SelfIterationSubagentStatus::CompletedNoWrite,
        SelfIterationWriteDecision::SkippedNoChange,
    ))
    .unwrap();

    SelfIterationReceiptStoreRuntime::append_record(&path, &reflection).unwrap();
    SelfIterationReceiptStoreRuntime::append_record(&path, &memory).unwrap();

    let receipts =
        SelfIterationReceiptStoreRuntime::receipts_for_parent_session(&path, "session-self")
            .unwrap();
    let latest =
        SelfIterationReceiptStoreRuntime::latest_for_parent_session(&path, "session-self").unwrap();
    let summary =
        SelfIterationReceiptStoreRuntime::summary_for_parent_session(&path, "session-self")
            .unwrap();
    let raw_file_text = ShardedJsonlStore::new()
        .read_lines(&path)
        .unwrap()
        .into_iter()
        .map(|line| String::from_utf8(line).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::remove_dir_all(root);

    assert_eq!(receipts.len(), 2);
    assert_eq!(receipts[0].hook, SelfIterationHookKind::MemoryUpdate);
    assert_eq!(receipts[1].hook, SelfIterationHookKind::SkillReflection);
    assert!(latest.is_some_and(|record| {
        record.prompt_mode == SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE
            && record.write_decision == SelfIterationWriteDecision::SkippedNoChange
    }));
    assert!(summary.is_some_and(|summary| {
        summary.total_receipts == 2
            && summary.memory_update_receipts == 1
            && summary.skill_reflection_receipts == 1
            && summary.persisted_write_receipts == 1
            && summary.total_cost_micros == 150
            && !summary.prompt_text_persisted
            && !summary.raw_transcript_persisted
            && !summary.remote_trigger_allowed
    }));
    assert!(!raw_file_text.contains("secret prompt"));
    assert!(!raw_file_text.contains("raw subagent transcript"));
}

#[test]
fn self_iteration_receipt_store_rejects_raw_payload_remote_or_mode_mismatch() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-self-iteration-receipts-invalid-test-{}",
        std::process::id()
    ));
    let path = SelfIterationReceiptStoreRuntime::resolve_store_path(&root);
    let mut raw_record = SelfIterationReceiptRecord::from_input(receipt_input(
        SelfIterationHookKind::MemoryUpdate,
        1_000,
        SelfIterationSubagentStatus::CompletedWithWrite,
        SelfIterationWriteDecision::Persisted,
    ))
    .unwrap();
    raw_record.raw_transcript_persisted = true;

    assert!(SelfIterationReceiptStoreRuntime::append_record(&path, &raw_record).is_err());

    let mut mode_mismatch = SelfIterationReceiptRecord::from_input(receipt_input(
        SelfIterationHookKind::MemoryUpdate,
        1_000,
        SelfIterationSubagentStatus::CompletedWithWrite,
        SelfIterationWriteDecision::Persisted,
    ))
    .unwrap();
    mode_mismatch.prompt_mode = SELF_ITERATION_SKILL_REFLECTION_PROMPT_MODE.to_owned();
    assert!(SelfIterationReceiptStoreRuntime::append_record(&path, &mode_mismatch).is_err());

    let mut invalid_schema = SelfIterationReceiptRecord::from_input(receipt_input(
        SelfIterationHookKind::MemoryUpdate,
        1_000,
        SelfIterationSubagentStatus::CompletedWithWrite,
        SelfIterationWriteDecision::Persisted,
    ))
    .unwrap();
    invalid_schema.schema_version = SELF_ITERATION_RECEIPT_STORE_SCHEMA_VERSION + 1;
    assert!(SelfIterationReceiptStoreRuntime::append_record(&path, &invalid_schema).is_err());
    let _ = std::fs::remove_dir_all(root);
}

fn receipt_input(
    hook: SelfIterationHookKind,
    recorded_at_ms: u64,
    status: SelfIterationSubagentStatus,
    write_decision: SelfIterationWriteDecision,
) -> SelfIterationReceiptInput {
    SelfIterationReceiptInput {
        parent_session_id: "session-self".to_owned(),
        parent_cell_id: Some(7),
        recorded_at_ms,
        hook,
        source_trace: "runtime-cell://session-self/7/self-iteration".to_owned(),
        status,
        cost_micros: match hook {
            SelfIterationHookKind::MemoryUpdate => 120,
            SelfIterationHookKind::SkillReflection => 30,
        },
        write_decision,
        reason: "subagent finished with bounded receipt".to_owned(),
        summary_hash: Some("self-iteration-summary-hash".to_owned()),
        skill_name: (hook == SelfIterationHookKind::SkillReflection)
            .then(|| "release-notes".to_owned()),
        hook_iterations: 2,
    }
}
