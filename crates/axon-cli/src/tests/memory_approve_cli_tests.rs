use std::{fs, path::Path};

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig as AxonMemoryConfig,
    MemoryOrchestrator, MemorySqliteRecord,
};
use axon_domain::{
    CellId, MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin,
    MemorySourceTrace, MemoryState, SessionId,
};
use serde_json::Value;

use super::cli_test_helpers::{test_root, write_default_config as write_config};
use super::{AxonCli, run_parsed_cli};

const BASE_NOW_MS: u64 = 1_800_000_000_000;

#[test]
fn memory_approve_promotes_weak_record() {
    let root = test_root("approve");
    let config_path = write_config(&root);
    seed_weak_memory(&root);

    let review_result = run_parsed_cli(&memory_cli(&config_path, &["review", "--limit", "4"]));
    assert_eq!(review_result.exit_code, 0, "{}", review_result.stderr);
    let review_body: Value =
        serde_json::from_str(&review_result.stdout).expect("memory review 应输出 JSON");
    assert_eq!(review_body["records"][0]["record"]["state"], "weak");
    assert!(
        review_body["records"][0]["recommendedActions"]
            .as_array()
            .expect("recommendedActions 应为数组")
            .iter()
            .any(|action| action == "promote")
    );

    let approve_result = run_parsed_cli(&memory_cli(&config_path, &["approve", "11"]));
    assert_eq!(approve_result.exit_code, 0, "{}", approve_result.stderr);
    let approve_body: Value =
        serde_json::from_str(&approve_result.stdout).expect("memory approve 应输出 JSON");

    assert_eq!(approve_body["status"], "memory_updated");
    assert_eq!(approve_body["action"], "approve");
    assert_eq!(approve_body["previousState"], "weak");
    assert_eq!(approve_body["record"]["state"], "strong");
    assert_eq!(
        approve_body["record"]["contentRef"],
        "memory-ref:command-renderer"
    );
    assert_eq!(approve_body["contentRefCleared"], false);
    assert_eq!(approve_body["vectorIndexCleared"], false);
    assert_eq!(approve_body["tombstoneWritten"], false);
    assert!(!approve_result.stdout.contains("secret-source-summary"));

    let strong_result = run_parsed_cli(&memory_cli(&config_path, &["list", "--kind", "strong"]));
    assert_eq!(strong_result.exit_code, 0, "{}", strong_result.stderr);
    let strong_body: Value =
        serde_json::from_str(&strong_result.stdout).expect("memory list 应输出 JSON");
    assert_eq!(strong_body["records"][0]["state"], "strong");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_approve_pending_requires_yes_then_batches_safe_weak_records() {
    let root = test_root("approve-pending");
    let config_path = write_config(&root);
    seed_batch_weak_memories(&root);

    let preview_result = run_parsed_cli(&memory_cli(
        &config_path,
        &["approve-pending", "--limit", "4"],
    ));
    assert_eq!(preview_result.exit_code, 0, "{}", preview_result.stderr);
    let preview_body: Value =
        serde_json::from_str(&preview_result.stdout).expect("approve-pending 应输出 JSON");
    assert_eq!(preview_body["status"], "memory_batch_preview");
    assert_eq!(preview_body["requiresYes"], true);
    assert_eq!(preview_body["approvedCount"], 2);
    assert_eq!(preview_body["skippedCount"], 1);

    let weak_preview = run_parsed_cli(&memory_cli(&config_path, &["list", "--kind", "weak"]));
    let weak_preview_body: Value =
        serde_json::from_str(&weak_preview.stdout).expect("memory list 应输出 JSON");
    assert_eq!(
        weak_preview_body["records"]
            .as_array()
            .expect("records 应为数组")
            .len(),
        3
    );

    let apply_result = run_parsed_cli(&memory_cli(
        &config_path,
        &["approve-pending", "--limit", "4", "--yes"],
    ));
    assert_eq!(apply_result.exit_code, 0, "{}", apply_result.stderr);
    let apply_body: Value =
        serde_json::from_str(&apply_result.stdout).expect("approve-pending 应输出 JSON");
    assert_eq!(apply_body["status"], "memory_batch_updated");
    assert_eq!(apply_body["requiresYes"], false);
    assert_eq!(apply_body["approvedCount"], 2);
    assert_eq!(apply_body["skippedCount"], 1);
    assert!(!apply_result.stdout.contains("secret-source-summary"));

    let strong_result = run_parsed_cli(&memory_cli(&config_path, &["list", "--kind", "strong"]));
    let strong_body: Value =
        serde_json::from_str(&strong_result.stdout).expect("memory list 应输出 JSON");
    assert_eq!(
        strong_body["records"]
            .as_array()
            .expect("records 应为数组")
            .len(),
        2
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_archive_pending_requires_yes_then_batches_pending_records() {
    let root = test_root("archive-pending");
    let config_path = write_config(&root);
    seed_batch_weak_memories(&root);

    let preview_result = run_parsed_cli(&memory_cli(
        &config_path,
        &["archive-pending", "--limit", "4"],
    ));
    assert_eq!(preview_result.exit_code, 0, "{}", preview_result.stderr);
    let preview_body: Value =
        serde_json::from_str(&preview_result.stdout).expect("archive-pending 应输出 JSON");
    assert_eq!(preview_body["status"], "memory_batch_preview");
    assert_eq!(preview_body["action"], "archive-pending");
    assert_eq!(preview_body["requiresYes"], true);
    assert_eq!(preview_body["archivedCount"], 3);
    assert_eq!(preview_body["skippedCount"], 0);

    let weak_preview = run_parsed_cli(&memory_cli(&config_path, &["list", "--kind", "weak"]));
    let weak_preview_body: Value =
        serde_json::from_str(&weak_preview.stdout).expect("memory list 应输出 JSON");
    assert_eq!(
        weak_preview_body["records"]
            .as_array()
            .expect("records 应为数组")
            .len(),
        3
    );

    let apply_result = run_parsed_cli(&memory_cli(
        &config_path,
        &["archive-pending", "--limit", "4", "--yes"],
    ));
    assert_eq!(apply_result.exit_code, 0, "{}", apply_result.stderr);
    let apply_body: Value =
        serde_json::from_str(&apply_result.stdout).expect("archive-pending 应输出 JSON");
    assert_eq!(apply_body["status"], "memory_batch_updated");
    assert_eq!(apply_body["requiresYes"], false);
    assert_eq!(apply_body["archivedCount"], 3);
    assert_eq!(apply_body["skippedCount"], 0);
    assert!(!apply_result.stdout.contains("secret-source-summary"));

    let quarantined_result = run_parsed_cli(&memory_cli(
        &config_path,
        &["list", "--kind", "quarantined"],
    ));
    let quarantined_body: Value =
        serde_json::from_str(&quarantined_result.stdout).expect("memory list 应输出 JSON");
    assert_eq!(
        quarantined_body["records"]
            .as_array()
            .expect("records 应为数组")
            .len(),
        3
    );
    assert!(
        quarantined_body["records"]
            .as_array()
            .expect("records 应为数组")
            .iter()
            .all(|record| record["contentRef"].is_null())
    );
    let _ = fs::remove_dir_all(root);
}

fn memory_cli(config_path: &Path, command: &[&str]) -> AxonCli {
    let mut args = vec![
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
    ];
    args.extend_from_slice(command);
    AxonCli::parse_from_args(args)
}

fn seed_weak_memory(root: &Path) {
    let orchestrator = memory_orchestrator(root);
    upsert_sqlite_record(&orchestrator, &weak_sqlite_record());
}

fn seed_batch_weak_memories(root: &Path) {
    let orchestrator = memory_orchestrator(root);
    for record in batch_weak_records() {
        upsert_sqlite_record(&orchestrator, &record);
    }
}

fn memory_orchestrator(root: &Path) -> MemoryOrchestrator {
    let config =
        AxonMemoryConfig::from_config_dir(root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    MemoryOrchestrator::init(config).expect("MemoryOrchestrator 应可初始化")
}

fn upsert_sqlite_record(orchestrator: &MemoryOrchestrator, record: &MemorySqliteRecord) {
    orchestrator
        .sqlite_store()
        .expect("SQLite store 应可打开")
        .upsert_record(record)
        .expect("weak memory 应可写入");
}

fn batch_weak_records() -> Vec<MemorySqliteRecord> {
    vec![
        weak_record(MemoryId(11), Some("memory-ref:command-renderer"), true),
        weak_record(MemoryId(12), Some("memory-ref:review-batch"), true),
        weak_record(MemoryId(13), Some("memory-ref:missing-source"), false),
    ]
}

fn weak_sqlite_record() -> MemorySqliteRecord {
    weak_record(MemoryId(11), Some("memory-ref:command-renderer"), true)
}

fn weak_record(
    id: MemoryId,
    content_ref: Option<&str>,
    with_source_trace: bool,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: MemoryKind::Knowledge,
        record: MemoryRecord {
            id,
            candidate_id: MemoryCandidateId(id.0),
            state: MemoryState::Weak,
            content_ref: content_ref.map(str::to_owned),
            source_trace: with_source_trace.then(|| MemorySourceTrace {
                origin: MemorySourceOrigin::DebuggingExperience,
                session_id: Some(SessionId(7)),
                cell_id: Some(CellId(70)),
                summary: "Memory 审批脱敏摘要".to_owned(),
                evidence_ref: Some("timeline:memory-approve-cli".to_owned()),
            }),
            tombstone: None,
        },
        namespace_ref: Some("memory-scope:test".to_owned()),
        repo_ref: Some("memory-repo:test".to_owned()),
        user_ref: None,
        created_at_ms: BASE_NOW_MS,
        updated_at_ms: BASE_NOW_MS + id.0,
        last_active_at_ms: BASE_NOW_MS + id.0,
        ttl_expires_at_ms: None,
        confidence_basis_points: 9_000,
        importance_basis_points: 8_000,
        vector_id: None,
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}
