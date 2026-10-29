use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig as AxonMemoryConfig,
    MemoryOrchestrator, MemorySqliteRecord, MemoryTextDocument, MemoryVectorDocument,
};
use axon_config::{AxonConfig, ConfigLoader};
#[cfg(feature = "memory-vector-index")]
use axon_config::{ModelConfig, ProviderConfig};
use axon_domain::{
    CellId, MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin,
    MemorySourceTrace, MemoryState, SessionId,
};
use axon_memory::{MemoryVectorProjection, memory_embedding_hash};
use serde_json::Value;

use super::{AxonCli, run_parsed_cli};
use crate::cli_test_helpers::{test_root, write_default_config as write_config};

const BASE_NOW_MS: u64 = 1_800_000_000_000;

#[test]
fn memory_consistency_checks_and_repairs_physical_projection_drift() {
    let root = test_root("consistency");
    let config_path = write_config(&root);
    seed_sqlite_only_and_partial_text_records(&root);

    let check_result = run_parsed_cli(&consistency_cli(&config_path, false));
    assert_eq!(check_result.exit_code, 0, "{}", check_result.stderr);
    let check_body: Value =
        serde_json::from_str(&check_result.stdout).expect("memory consistency 应输出 JSON");

    assert_eq!(check_body["status"], "memory_consistency_ready");
    assert_eq!(check_body["check"]["sqliteRecallableRecords"], 2);
    assert_eq!(check_body["check"]["checkedRecords"], 2);
    assert_eq!(check_body["check"]["probeBudgetRecords"], 8);
    assert_eq!(
        check_body["check"]["textIndexMissingRecords"],
        expected_text_index_missing_records(1)
    );
    assert_eq!(check_body["check"]["textIndexUnreadable"], false);
    assert_eq!(
        check_body["check"]["repairRecommended"],
        cfg!(feature = "memory-text-index")
    );
    assert_eq!(
        check_body["check"]["operatorAction"],
        expected_consistency_action()
    );
    assert_eq!(check_body["repair"], Value::Null);
    assert!(!check_result.stdout.contains("secret-source-summary"));

    let repair_result = run_parsed_cli(&consistency_cli(&config_path, true));
    assert_eq!(repair_result.exit_code, 0, "{}", repair_result.stderr);
    let repair_body: Value =
        serde_json::from_str(&repair_result.stdout).expect("memory consistency repair 应输出 JSON");

    assert_eq!(
        repair_body["status"],
        expected_consistency_status_after_repair()
    );
    assert_eq!(repair_body["check"]["textIndexMissingRecords"], 0);
    assert_eq!(repair_body["check"]["textIndexUnreadable"], false);
    assert_eq!(repair_body["check"]["repairRecommended"], false);
    assert_eq!(repair_body["check"]["operatorAction"], "none");
    if cfg!(feature = "memory-text-index") {
        assert_eq!(repair_body["repair"]["textIndexRebuilt"], 2);
        assert_eq!(repair_body["repair"]["refOnly"], true);
        assert_eq!(repair_body["repair"]["remoteWriteAllowed"], false);
    } else {
        assert_eq!(repair_body["repair"], Value::Null);
    }
    assert!(!repair_result.stdout.contains("secret-source-summary"));
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "memory-vector-index")]
fn memory_consistency_repairs_vector_index_from_embedding_projection() {
    let root = test_root("consistency-corrupt-hnsw");
    let config_path = write_embedding_config(&root);
    seed_vector_projection_records(&root);
    let config =
        AxonMemoryConfig::from_config_dir(&root, true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    fs::write(&config.hnsw_index_path, b"broken-hnsw").expect("测试 HNSW 应可写坏");

    let check_result = run_parsed_cli(&consistency_cli(&config_path, false));
    assert_eq!(check_result.exit_code, 0, "{}", check_result.stderr);
    let check_body: Value =
        serde_json::from_str(&check_result.stdout).expect("memory consistency 应输出 JSON");

    assert_eq!(check_body["status"], "memory_consistency_ready");
    assert_eq!(check_body["check"]["vectorIndexUnreadable"], true);
    assert_eq!(check_body["check"]["vectorIndexRecords"], 0);
    assert_eq!(check_body["check"]["vectorRecordsDeferred"], 0);
    assert_eq!(
        check_body["check"]["operatorAction"],
        "run_memory_consistency_repair"
    );
    assert_eq!(check_body["repair"], Value::Null);
    assert!(!check_result.stdout.contains("secret-source-summary"));

    fs::remove_file(&config.hnsw_index_path).expect("测试 HNSW 坏文件应可清理");
    let repair_result = run_parsed_cli(&consistency_cli(&config_path, true));
    assert_eq!(repair_result.exit_code, 0, "{}", repair_result.stderr);
    let repair_body: Value =
        serde_json::from_str(&repair_result.stdout).expect("memory consistency repair 应输出 JSON");

    assert_eq!(repair_body["status"], "memory_consistency_repaired");
    assert_eq!(repair_body["repair"]["vectorIndexRebuilt"], 1);
    assert_eq!(repair_body["repair"]["vectorRecordsDeferred"], 0);
    assert_eq!(repair_body["check"]["vectorIndexUnreadable"], false);
    assert_eq!(repair_body["check"]["vectorIndexMissingRecords"], 0);
    assert_eq!(repair_body["check"]["repairRecommended"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
#[cfg(feature = "memory-text-index")]
fn memory_consistency_repairs_unreadable_text_index_from_sqlite_authority() {
    let root = test_root("consistency-corrupt-text");
    let config_path = write_config(&root);
    seed_sqlite_only_and_partial_text_records(&root);
    let config =
        AxonMemoryConfig::from_config_dir(&root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    fs::remove_dir_all(&config.tantivy_index_dir).expect("测试 Tantivy 目录应可清理");
    fs::write(&config.tantivy_index_dir, b"blocked").expect("测试 Tantivy 路径应可占用");

    let check_result = run_parsed_cli(&consistency_cli(&config_path, false));
    assert_eq!(check_result.exit_code, 0, "{}", check_result.stderr);
    let check_body: Value =
        serde_json::from_str(&check_result.stdout).expect("memory consistency 应输出 JSON");

    assert_eq!(check_body["status"], "memory_consistency_ready");
    assert_eq!(check_body["check"]["textIndexUnreadable"], true);
    assert_eq!(check_body["check"]["textIndexMissingRecords"], 0);
    assert_eq!(check_body["check"]["repairRecommended"], true);
    assert_eq!(
        check_body["check"]["operatorAction"],
        "run_memory_consistency_repair"
    );
    assert_eq!(check_body["repair"], Value::Null);
    fs::remove_file(&config.tantivy_index_dir).expect("测试 Tantivy 占用文件应可清理");

    let repair_result = run_parsed_cli(&consistency_cli(&config_path, true));
    assert_eq!(repair_result.exit_code, 0, "{}", repair_result.stderr);
    let repair_body: Value =
        serde_json::from_str(&repair_result.stdout).expect("memory consistency repair 应输出 JSON");

    assert_eq!(repair_body["status"], "memory_consistency_repaired");
    assert_eq!(repair_body["check"]["textIndexUnreadable"], false);
    assert_eq!(repair_body["check"]["textIndexMissingRecords"], 0);
    assert_eq!(repair_body["check"]["repairRecommended"], false);
    assert_eq!(repair_body["repair"]["textIndexRebuilt"], 2);
    assert!(!repair_result.stdout.contains("secret-source-summary"));
    let _ = fs::remove_dir_all(root);
}

fn consistency_cli(config_path: &Path, repair: bool) -> AxonCli {
    let mut args = vec![
        "axon",
        "--config",
        config_path.to_str().expect("测试路径应为 UTF-8"),
        "--format",
        "json",
        "memory",
        "consistency",
        "--max-probe-records",
        "8",
    ];
    if repair {
        args.push("--repair");
    }
    AxonCli::parse_from_args(args)
}

const fn expected_text_index_missing_records(indexed_expected: usize) -> usize {
    if cfg!(feature = "memory-text-index") {
        indexed_expected
    } else {
        0
    }
}

const fn expected_consistency_action() -> &'static str {
    if cfg!(feature = "memory-text-index") {
        "run_memory_consistency_repair"
    } else {
        "none"
    }
}

const fn expected_consistency_status_after_repair() -> &'static str {
    if cfg!(feature = "memory-text-index") {
        "memory_consistency_repaired"
    } else {
        "memory_consistency_ready"
    }
}

fn seed_sqlite_only_and_partial_text_records(root: &Path) {
    let config =
        AxonMemoryConfig::from_config_dir(root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    let orchestrator = MemoryOrchestrator::init(config).expect("MemoryOrchestrator 应可初始化");
    let store = orchestrator.sqlite_store().expect("SQLite store 应可打开");
    let index = orchestrator.text_index().expect("Tantivy index 应可打开");
    let indexed = sqlite_record(MemoryId(50), "memory-ref:consistency indexed");
    let missing = sqlite_record(MemoryId(51), "memory-ref:consistency missing text");
    store
        .upsert_record(&indexed)
        .expect("indexed memory 应可写入 SQLite");
    store
        .upsert_record(&missing)
        .expect("missing memory 应可写入 SQLite");
    index
        .upsert_document(&MemoryTextDocument {
            memory_id: indexed.record.id,
            content_ref: indexed.record.content_ref.clone(),
            evidence_ref: indexed
                .record
                .source_trace
                .as_ref()
                .and_then(|trace| trace.evidence_ref.clone()),
            tombstone_ref: None,
            memory_summary: Some("memory consistency cli summary".to_owned()),
        })
        .expect("indexed memory 文本索引应可写入");
}

#[cfg(feature = "memory-vector-index")]
fn seed_vector_projection_records(root: &Path) {
    let config =
        AxonMemoryConfig::from_config_dir(root, true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .expect("MemoryConfig 应可创建");
    let orchestrator = MemoryOrchestrator::init(config).expect("MemoryOrchestrator 应可初始化");
    let store = orchestrator.sqlite_store().expect("SQLite store 应可打开");
    let vector_id = 70_i64;
    let mut record = sqlite_record(MemoryId(70), "memory-ref:consistency vector");
    record.vector_id = Some(vector_id);
    store
        .upsert_record(&record)
        .expect("向量记录应可写入 SQLite");
    let embedding = projection_embedding();
    orchestrator
        .vector_projection_store()
        .expect("向量投影 store 应可打开")
        .upsert_projection(&MemoryVectorProjection {
            memory_id: record.record.id,
            vector_id,
            model_ref: "text-embedding-3-small".to_owned(),
            dimension: MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
            embedding_hash: memory_embedding_hash(&embedding),
            embedding: embedding.clone(),
            updated_at_ms: BASE_NOW_MS,
        })
        .expect("向量投影应可写入");
    orchestrator
        .vector_index()
        .expect("HNSW 应可打开")
        .expect("embedding 已启用")
        .upsert_vector(MemoryVectorDocument {
            vector_id,
            embedding,
        })
        .expect("HNSW 初始向量应可写入");
}

#[cfg(feature = "memory-vector-index")]
fn projection_embedding() -> Vec<f32> {
    let mut embedding = vec![0.0; MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION];
    embedding[0] = 1.0;
    embedding
}

fn sqlite_record(id: MemoryId, content_ref: &str) -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: MemoryKind::Knowledge,
        record: MemoryRecord {
            id,
            candidate_id: MemoryCandidateId(id.0),
            state: MemoryState::Strong,
            content_ref: Some(content_ref.to_owned()),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::DebuggingExperience,
                session_id: Some(SessionId(7)),
                cell_id: Some(CellId(70)),
                summary: "Memory 一致性脱敏摘要".to_owned(),
                evidence_ref: Some("timeline:memory-consistency-cli".to_owned()),
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

#[cfg(feature = "memory-vector-index")]
fn write_embedding_config(root: &Path) -> PathBuf {
    fs::create_dir_all(root).expect("测试配置目录应可创建");
    let mut config = AxonConfig::default();
    config.providers.push(ProviderConfig::new(
        "openai",
        "openai",
        "https://api.openai.com/v1",
    ));
    config.models.push(ModelConfig {
        name: String::new(),
        id: "text-embedding-3-small".to_owned(),
        provider: "openai".to_owned(),
        context_window: 0,
        max_output_tokens: None,
        supports_vision: false,
        supports_embedding: true,
        embedding_dimension: MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
    requires_reasoning_content_on_assistant_messages: false,
    });
    "text-embedding-3-small".clone_into(&mut config.agent.default_model);
    "text-embedding-3-small".clone_into(&mut config.agent.fast_model);
    config.memory.embedding.enabled = true;
    "text-embedding-3-small".clone_into(&mut config.memory.embedding.model);
    config.memory.embedding.dimension = MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION;
    let config_path = root.join("config.toml");
    ConfigLoader::save_to_path(&config_path, &config).expect("测试 embedding 配置应可写入");
    config_path
}
