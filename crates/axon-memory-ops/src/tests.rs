use std::path::Path;

use axon_agent_runtime::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig, MemoryOrchestrator, MemorySqliteRecord,
};
use axon_domain::{
    CellId, MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin,
    MemorySourceTrace, MemoryState, SessionId,
};

use super::*;

#[test]
fn export_import_v3_preserves_summary_scope_and_kind() {
    let root = axon_test_support::test_temp_root()
        .join(format!("axon-memory-ops-roundtrip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let source = context(&root.join("source"));
    let target = context(&root.join("target"));
    source
        .orchestrator
        .sqlite_store()
        .unwrap()
        .upsert_record(&record())
        .unwrap();
    let bundle_path = root.join("memory-export.json");

    let exported = run_memory_export(
        MemoryOpsOutputFormat::Json,
        &source,
        &MemoryExportCommand {
            limit: Some(4),
            output: Some(bundle_path.clone()),
            signing_key_id: None,
            signing_seed_hex: None,
            encryption_key_id: None,
            encryption_key_hex: None,
        },
    );
    assert_eq!(exported.exit_code, 0, "{}", exported.stderr);
    let bundle: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bundle_path).unwrap()).unwrap();
    assert_eq!(bundle["schemaVersion"], MEMORY_EXPORT_SCHEMA_VERSION);
    assert_eq!(bundle["memorySummaryExported"], true);

    let imported = run_memory_import(
        MemoryOpsOutputFormat::Json,
        &target,
        &MemoryImportCommand {
            input: bundle_path,
            yes: true,
            trusted_public_key_hex: None,
            decryption_key_hex: None,
        },
    );
    assert_eq!(imported.exit_code, 0, "{}", imported.stderr);
    let restored = target
        .orchestrator
        .sqlite_store()
        .unwrap()
        .get_record(MemoryId(42))
        .unwrap()
        .expect("导入后记录应存在");
    assert_eq!(
        restored
            .record
            .source_trace
            .as_ref()
            .map(|trace| trace.summary.as_str()),
        Some("Rust 文件必须使用 UTF-8 无 BOM")
    );
    assert_eq!(restored.namespace_ref.as_deref(), Some("namespace:test"));
    assert_eq!(restored.repo_ref.as_deref(), Some("repo:test"));
    assert_eq!(restored.user_ref.as_deref(), Some("user:test"));
    assert_eq!(restored.memory_kind, MemoryKind::Preference);
    let _ = std::fs::remove_dir_all(root);
}

fn context(root: &Path) -> MemoryOpsContext {
    let config =
        MemoryConfig::from_config_dir(root, false, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap();
    MemoryOpsContext {
        orchestrator: MemoryOrchestrator::init(config).unwrap(),
    }
}

fn record() -> MemorySqliteRecord {
    MemorySqliteRecord {
        memory_kind: MemoryKind::Preference,
        record: MemoryRecord {
            id: MemoryId(42),
            candidate_id: MemoryCandidateId(42),
            state: MemoryState::Strong,
            content_ref: Some("memory-ref:encoding-policy".to_owned()),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::ExplicitUserPreference,
                session_id: Some(SessionId(1)),
                cell_id: Some(CellId(42)),
                summary: "Rust 文件必须使用 UTF-8 无 BOM".to_owned(),
                evidence_ref: Some("memory://42".to_owned()),
            }),
            tombstone: None,
        },
        namespace_ref: Some("namespace:test".to_owned()),
        repo_ref: Some("repo:test".to_owned()),
        user_ref: Some("user:test".to_owned()),
        created_at_ms: 1_000,
        updated_at_ms: 1_000,
        last_active_at_ms: 1_000,
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
