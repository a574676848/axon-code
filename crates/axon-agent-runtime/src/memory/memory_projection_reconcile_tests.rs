//! Memory 物理投影反向核对（孤儿清理）测试。
//!
//! 覆盖历史缺陷：旧版本直接重建 `memory_records` 会留下无主的向量投影与文本文档，
//! 而「只从权威表重建」的修复路径永远发现不了这类残留。

use axon_domain::{
    MemoryCandidateId, MemoryId, MemoryKind, MemoryRecord, MemorySourceOrigin, MemorySourceTrace,
    MemoryState, SessionId,
};
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig, MemorySqliteRecord,
    MemoryVectorDocument, MemoryVectorProjection, MemoryVectorProjectionStore,
};

use crate::{
    MemoryOrchestrator, MemoryPhysicalConsistencyCheckInput, MemoryPhysicalConsistencyCheckRuntime,
    MemoryPhysicalConsistencyRepairInput, MemoryPhysicalConsistencyRepairRuntime,
};

const NOW_MS: u64 = 1_700_000_000_000;

fn record(id: u64, namespace: &str) -> MemorySqliteRecord {
    MemorySqliteRecord {
        record: MemoryRecord {
            id: MemoryId(id),
            candidate_id: MemoryCandidateId(id),
            state: MemoryState::Strong,
            content_ref: Some(format!("memory-tool:ref-{id}")),
            source_trace: Some(MemorySourceTrace {
                origin: MemorySourceOrigin::AgentKnowledge,
                session_id: Some(SessionId(1)),
                cell_id: None,
                summary: format!("孤儿核对样本 {id}"),
                evidence_ref: Some(format!("evidence:{id}")),
            }),
            tombstone: None,
        },
        memory_kind: MemoryKind::Knowledge,
        namespace_ref: Some(namespace.to_owned()),
        repo_ref: None,
        user_ref: None,
        created_at_ms: NOW_MS - 1_000,
        updated_at_ms: NOW_MS - 1_000,
        last_active_at_ms: NOW_MS - 1_000,
        ttl_expires_at_ms: None,
        confidence_basis_points: 9_000,
        importance_basis_points: 8_000,
        vector_id: Some(id as i64),
        archived_at_ms: None,
        prompt_text_persisted: false,
        raw_payload_persisted: false,
        remote_write_allowed: false,
    }
}

#[test]
fn orphan_projection_and_text_residue_are_detected_and_pruned() {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-memory-orphan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let config =
        MemoryConfig::from_config_dir(&root, true, MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION)
            .unwrap();
    let orchestrator = MemoryOrchestrator::init(config.clone()).unwrap();
    let store = orchestrator.sqlite_store().unwrap();
    let surviving = record(7_001, "workspace:orphan-test");
    let wiped = record(7_002, "workspace:orphan-test");
    store.upsert_record(&surviving).unwrap();
    store.upsert_record(&wiped).unwrap();

    // 写入两条投影与两条文本文档，然后模拟旧版本行为：只删记录行，不动投影。
    let projection_store = MemoryVectorProjectionStore::open(&config).unwrap();
    projection_store.initialize_schema().unwrap();
    let embedding = vec![0.5_f32; MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION];
    for item in [&surviving, &wiped] {
        projection_store
            .upsert_projection(&MemoryVectorProjection {
                memory_id: item.record.id,
                vector_id: item.vector_id.unwrap(),
                model_ref: "embedding:test".to_owned(),
                dimension: MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION,
                embedding_hash: axon_memory::memory_embedding_hash(&embedding),
                embedding: embedding.clone(),
                updated_at_ms: NOW_MS - 1_000,
            })
            .unwrap();
    }
    if let Some(index) = orchestrator.vector_index().unwrap() {
        index
            .replace_vectors([
                MemoryVectorDocument {
                    vector_id: surviving.vector_id.unwrap(),
                    embedding: embedding.clone(),
                },
                MemoryVectorDocument {
                    vector_id: wiped.vector_id.unwrap(),
                    embedding: embedding.clone(),
                },
            ])
            .unwrap();
    }
    store.delete_records(&[wiped.record.id]).unwrap();

    let check = MemoryPhysicalConsistencyCheckRuntime
        .check(&MemoryPhysicalConsistencyCheckInput {
            orchestrator: &orchestrator,
            now_ms: NOW_MS,
            max_probe_records: 64,
        })
        .unwrap();
    assert_eq!(check.orphan_projection_records, 1, "必须发现无主向量投影");
    assert!(check.repair_recommended, "发现残留时必须建议修复");

    let repair = MemoryPhysicalConsistencyRepairRuntime
        .repair(&MemoryPhysicalConsistencyRepairInput {
            orchestrator: &orchestrator,
            now_ms: NOW_MS,
            max_repair_records: 1,
        })
        .unwrap();
    assert_eq!(repair.orphan_projections_pruned, 1, "孤儿投影必须被清理");
    // 文本文档的孤儿清理只在启用文本索引特性时存在：默认特性下这条路径不运行，
    // 断言必须跟着特性走，否则单跑本 crate 会稳定失败（--workspace 因特性统一而通过）。
    #[cfg(feature = "memory-text-index")]
    assert!(
        repair.orphan_text_documents_pruned >= 1,
        "孤儿文本文档必须被清理"
    );
    #[cfg(not(feature = "memory-text-index"))]
    assert_eq!(
        repair.orphan_text_documents_pruned, 0,
        "未启用文本索引时不存在文本文档清理路径"
    );

    // 修复窗口只有 1 条记录，但物理向量索引是全量替换语义：
    // 窗口之外的向量不得被丢掉。
    if let Some(index) = orchestrator.vector_index().unwrap() {
        assert_eq!(
            index.len().unwrap(),
            1,
            "全量替换必须保留窗口之外仍然有效的向量"
        );
        assert!(
            index
                .contains_vector_id(surviving.vector_id.unwrap())
                .unwrap()
        );
        assert!(!index.contains_vector_id(wiped.vector_id.unwrap()).unwrap());
    }

    let after = MemoryPhysicalConsistencyCheckRuntime
        .check(&MemoryPhysicalConsistencyCheckInput {
            orchestrator: &orchestrator,
            now_ms: NOW_MS,
            max_probe_records: 64,
        })
        .unwrap();
    assert_eq!(after.orphan_projection_records, 0);
    assert_eq!(after.orphan_text_documents, 0);
    assert!(!after.repair_recommended);

    drop(projection_store);
    drop(store);
    let _ = std::fs::remove_dir_all(root);
}
