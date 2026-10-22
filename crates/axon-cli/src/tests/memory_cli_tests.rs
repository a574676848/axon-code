use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig as AxonMemoryConfig,
    MemoryOrchestrator, MemorySqliteRecord, MemoryTextDocument,
};
use axon_config::{AxonConfig, ConfigLoader};
use axon_domain::{
    CellId, MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin,
    MemorySourceTrace, MemoryState, SessionId,
};
use serde_json::Value;

use super::cli_test_helpers::test_root;
pub(super) use super::{AxonCli, run_parsed_cli};

const TEST_RECORD_ORDERING_WINDOW_MS: u64 = 1_000;

#[path = "memory_cli_query_tests.rs"]
mod memory_cli_query_tests;
#[path = "memory_consistency_cli_tests.rs"]
mod memory_consistency_cli_tests;

#[test]
fn memory_consolidate_deduplicates_sqlite_records_without_raw_text() {
    let root = test_root("consolidate");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_duplicate_sqlite_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "consolidate",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory consolidate 应输出 JSON");

    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(body["status"], "memory_consolidation_ready");
    assert_eq!(body["duplicateTombstones"], 1);
    assert_eq!(body["explanations"][0]["action"], "duplicate_merge");
    assert_eq!(body["explanations"][0]["affectedRecords"], 1);
    assert!(
        body["explanations"][0]["summary"]
            .as_str()
            .is_some_and(|summary| summary.contains("重复"))
    );
    assert_eq!(body["refOnly"], true);
    assert_eq!(body["remoteWriteAllowed"], false);
    assert!(!result.stdout.contains("secret-source-summary"));
    let store = memory_orchestrator(&root)
        .sqlite_store()
        .expect("SQLite store 应可打开");
    let loser = store
        .get_record(MemoryId(20))
        .expect("SQLite 记录应可读取")
        .expect("重复 loser 应存在");
    let winner = store
        .get_record(MemoryId(21))
        .expect("SQLite 记录应可读取")
        .expect("重复 winner 应存在");
    assert_eq!(loser.record.state, MemoryState::Tombstone);
    assert!(loser.record.content_ref.is_none());
    assert!(loser.archived_at_ms.is_some());
    assert_eq!(winner.record.state, MemoryState::Strong);
    assert_eq!(
        winner.record.content_ref.as_deref(),
        Some("memory-ref:duplicate-cli")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_recover_conflict_reactivates_archived_sqlite_record() {
    let root = test_root("recover-conflict");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_conflicting_sqlite_records(&root);

    let consolidate_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "consolidate",
    ]);
    let consolidate_result = run_parsed_cli(&consolidate_cli);
    assert_eq!(
        consolidate_result.exit_code, 0,
        "{}",
        consolidate_result.stderr
    );

    let recover_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "recover-conflict",
        "30",
    ]);
    let result = run_parsed_cli(&recover_cli);
    let body: Value =
        serde_json::from_str(&result.stdout).expect("memory recover-conflict 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "memory_conflict_recovered");
    assert_eq!(body["memoryId"], 30);
    assert_eq!(body["previousStatus"], "archived");
    assert_eq!(body["currentStatus"], "active");
    assert_eq!(body["refOnly"], true);
    assert_eq!(body["remoteWriteAllowed"], false);
    assert!(!result.stdout.contains("secret-source-summary"));
    let record = memory_orchestrator(&root)
        .sqlite_store()
        .expect("SQLite store 应可打开")
        .get_record(MemoryId(30))
        .expect("SQLite 记录应可读取")
        .expect("恢复记录应存在");
    assert!(record.archived_at_ms.is_none());
    assert!(record.record.tombstone.is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_conflicts_lists_ref_only_conflict_groups() {
    let root = test_root("conflicts");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_conflicting_sqlite_records(&root);

    let list_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "conflicts",
        "--limit",
        "4",
    ]);
    let list_result = run_parsed_cli(&list_cli);
    let list_body: Value =
        serde_json::from_str(&list_result.stdout).expect("memory conflicts 应输出 JSON");

    assert_eq!(list_result.exit_code, 0, "{}", list_result.stderr);
    assert_eq!(list_body["status"], "memory_conflicts_ready");
    assert_eq!(list_body["conflictGroupCount"], 1);
    assert_eq!(
        list_body["groups"][0]["records"].as_array().unwrap().len(),
        2
    );
    assert_eq!(list_body["refOnly"], true);
    assert_eq!(list_body["remoteWriteAllowed"], false);
    assert!(!list_result.stdout.contains("secret-source-summary"));

    let consolidate_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "consolidate",
    ]);
    assert_eq!(run_parsed_cli(&consolidate_cli).exit_code, 0);
    let archived_result = run_parsed_cli(&list_cli);
    let archived_body: Value =
        serde_json::from_str(&archived_result.stdout).expect("memory conflicts 应输出 JSON");
    assert!(
        archived_body["groups"][0]["records"]
            .as_array()
            .unwrap()
            .iter()
            .any(|record| record["status"] == "archived" && record["recoverableConflict"] == true)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_quality_reports_gate_metrics_without_mutating_store() {
    let root = test_root("quality");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_duplicate_sqlite_records(&root);
    seed_conflicting_sqlite_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "quality",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory quality 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "memory_quality_ready");
    assert_eq!(body["totalRecords"], 4);
    assert_eq!(body["staleFactResidualRecords"], 1);
    assert_eq!(body["staleFactBasisPoints"], 2_500);
    assert_eq!(body["duplicateActiveResidualRecords"], 1);
    assert_eq!(body["duplicateActiveResidualBasisPoints"], 2_500);
    assert_eq!(body["qualityGatePassed"], false);
    assert_eq!(body["refOnly"], true);
    assert!(!result.stdout.contains("secret-source-summary"));
    let records = memory_orchestrator(&root)
        .sqlite_store()
        .expect("SQLite store 应可打开")
        .records()
        .expect("quality 后 SQLite store 应可读取");
    assert_eq!(records.len(), 4);
    assert!(records.iter().all(|record| {
        record.archived_at_ms.is_none() && record.record.state == MemoryState::Strong
    }));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn memory_quarantine_persists_ref_only_state_and_blocks_search() {
    let root = test_root("quarantine");
    let config_path = write_config(&root, &AxonConfig::default());
    seed_memory_records(&root);

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "quarantine",
        "10",
        "--reason",
        "local-review",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("memory quarantine 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "memory_updated");
    assert_eq!(body["action"], "quarantine");
    assert_eq!(body["previousState"], "strong");
    assert_eq!(body["record"]["state"], "quarantined");
    assert_eq!(body["record"]["contentRef"], Value::Null);
    assert_eq!(
        body["record"]["tombstoneBlockedRef"],
        "memory-ref:中文 检索 调试 经验"
    );
    assert_eq!(body["contentRefCleared"], true);
    assert_eq!(body["vectorIndexCleared"], true);
    assert!(!result.stdout.contains("secret-source-summary"));

    let search = AxonCli::parse_from_args([
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "search",
        "中文 检索",
    ]);
    let search_result = run_parsed_cli(&search);
    let search_body: Value =
        serde_json::from_str(&search_result.stdout).expect("memory search 应输出 JSON");
    assert_eq!(
        search_body["hits"].as_array().expect("hits 应为数组").len(),
        0
    );
    let _ = fs::remove_dir_all(root);
}

pub(super) fn write_config(root: &Path, config: &AxonConfig) -> PathBuf {
    fs::create_dir_all(root).expect("测试配置目录应可创建");
    let config_path = root.join("config.toml");
    ConfigLoader::save_to_path(&config_path, config).expect("测试配置应可写入");
    config_path
}

pub(super) fn seed_memory_records(root: &Path) {
    let base_now_ms = current_record_base_ms();
    let config =
        AxonMemoryConfig::from_config_dir(root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    let orchestrator = MemoryOrchestrator::init(config).expect("MemoryOrchestrator 应可初始化");
    let store = orchestrator.sqlite_store().expect("SQLite store 应可打开");
    let index = orchestrator.text_index().expect("Tantivy index 应可打开");
    let records = [
        sqlite_record(
            MemoryId(10),
            MemoryState::Strong,
            Some("memory-ref:中文 检索 调试 经验"),
            base_now_ms + 5,
        ),
        sqlite_record(
            MemoryId(11),
            MemoryState::Weak,
            Some("memory-ref:command-renderer"),
            base_now_ms + 4,
        ),
        sqlite_record(
            MemoryId(12),
            MemoryState::Strong,
            Some("memory-ref:auto-write-skipped-receipt"),
            base_now_ms + 3,
        ),
        sqlite_record(
            MemoryId(13),
            MemoryState::Strong,
            Some("memory-ref:footer-governance-status"),
            base_now_ms + 2,
        ),
        sqlite_record(
            MemoryId(14),
            MemoryState::Weak,
            Some("memory-ref:review-quarantine-policy"),
            base_now_ms + 1,
        ),
        sqlite_record(
            MemoryId(15),
            MemoryState::Quarantined,
            Some("memory-ref:quarantined-secret"),
            base_now_ms,
        ),
    ];
    let documents = records.iter().map(memory_text_document).collect::<Vec<_>>();
    for record in &records {
        store
            .upsert_record(record)
            .expect("memory golden seed 应可写入");
    }
    index
        .upsert_documents(&documents)
        .expect("memory golden 文本索引应可写入");
}

fn memory_text_document(record: &MemorySqliteRecord) -> MemoryTextDocument {
    MemoryTextDocument {
        memory_id: record.record.id,
        content_ref: record.record.content_ref.clone(),
        evidence_ref: record
            .record
            .source_trace
            .as_ref()
            .and_then(|trace| trace.evidence_ref.clone()),
        tombstone_ref: record
            .record
            .tombstone
            .as_ref()
            .map(|tombstone| tombstone.blocked_ref.clone()),
        memory_summary: Some("memory cli test summary".to_owned()),
    }
}

fn seed_duplicate_sqlite_records(root: &Path) {
    seed_governance_records(
        root,
        &[
            (MemoryId(20), "memory-ref:duplicate-cli"),
            (MemoryId(21), "memory-ref:duplicate-cli"),
        ],
        SessionId(7),
        CellId(70),
        "timeline:memory-cli-duplicate",
    );
}

fn seed_conflicting_sqlite_records(root: &Path) {
    seed_governance_records(
        root,
        &[
            (MemoryId(30), "memory-ref:quality-cli-old"),
            (MemoryId(31), "memory-ref:quality-cli-new"),
        ],
        SessionId(8),
        CellId(80),
        "timeline:memory-cli-quality-conflict",
    );
}

fn seed_governance_records(
    root: &Path,
    records: &[(MemoryId, &str)],
    session_id: SessionId,
    cell_id: CellId,
    evidence_ref: &str,
) {
    let base_now_ms = current_record_base_ms();
    let orchestrator = memory_orchestrator(root);
    let store = orchestrator.sqlite_store().expect("SQLite store 应可打开");
    for (id, content_ref) in records {
        let mut record = sqlite_record(
            *id,
            MemoryState::Strong,
            Some(content_ref),
            base_now_ms + id.0,
        );
        record.record.source_trace = Some(MemorySourceTrace {
            origin: MemorySourceOrigin::DebuggingExperience,
            session_id: Some(session_id),
            cell_id: Some(cell_id),
            summary: "Memory CLI 治理脱敏摘要".to_owned(),
            evidence_ref: Some(evidence_ref.to_owned()),
        });
        store
            .upsert_record(&record)
            .expect("SQLite 治理记录应可写入");
    }
}

fn memory_orchestrator(root: &Path) -> MemoryOrchestrator {
    let config =
        AxonMemoryConfig::from_config_dir(root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    MemoryOrchestrator::init(config).expect("MemoryOrchestrator 应可初始化")
}

fn current_record_base_ms() -> u64 {
    axon_time::unix_now_ms().saturating_sub(TEST_RECORD_ORDERING_WINDOW_MS)
}

fn sqlite_record(
    id: MemoryId,
    state: MemoryState,
    content_ref: Option<&str>,
    updated_at_ms: u64,
) -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: MemoryKind::Knowledge,
        record: MemoryRecord {
            id,
            candidate_id: MemoryCandidateId(id.0),
            state,
            content_ref: content_ref.map(str::to_owned),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::DebuggingExperience,
                session_id: Some(SessionId(7)),
                cell_id: Some(CellId(70)),
                summary: "Memory CLI 脱敏测试摘要".to_owned(),
                evidence_ref: Some("timeline:memory-cli".to_owned()),
            }),
            tombstone: None,
        },
        namespace_ref: Some("memory-scope:test".to_owned()),
        repo_ref: Some("memory-repo:test".to_owned()),
        user_ref: None,
        created_at_ms: updated_at_ms,
        updated_at_ms,
        last_active_at_ms: updated_at_ms,
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
