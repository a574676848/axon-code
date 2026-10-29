use axon_domain::MemoryId;

use super::*;

#[test]
fn citation_parse_reports_valid_memory_refs() {
    let report = parse_memory_citations(
        "<oai-mem-citation><citation_entries>memory:42|note=[ok]</citation_entries></oai-mem-citation>",
    );

    assert_eq!(report.citations.len(), 1);
    assert_eq!(report.citations[0].memory_id, MemoryId(42));
}

#[test]
fn turn_citation_report_splits_cited_and_adopted_hits() {
    let context = MemoryTurnContext {
        surface: "test".to_owned(),
        recalled_hits: vec![
            turn_hit(7, "memory-ref:cited"),
            turn_hit(8, "memory-ref:scanned"),
        ],
        injected_hits: vec![
            turn_hit(7, "memory-ref:cited"),
            turn_hit(8, "memory-ref:scanned"),
        ],
    };

    let report = memory_turn_citation_report(
        &context,
        "<oai-mem-citation><citation_entries>
memory:7|note=[used]
memory:99|note=[not injected]
</citation_entries></oai-mem-citation>",
        "tool receipt mentioned memory-ref:scanned",
    );

    assert_eq!(report.cited_memory_ids, [7]);
    assert_eq!(report.cited_hit_refs, ["memory-ref:cited"]);
    assert_eq!(
        report.adopted_hit_refs,
        ["memory-ref:cited", "memory-ref:scanned"]
    );
}

#[test]
fn turn_cited_usage_events_are_strict_model_citations_only() {
    let context = MemoryTurnContext {
        surface: "test".to_owned(),
        recalled_hits: vec![turn_hit(7, "memory-ref:cited")],
        injected_hits: vec![turn_hit(7, "memory-ref:cited")],
    };

    let events = memory_turn_cited_usage_events(
        &context,
        "<oai-mem-citation><citation_entries>
memory:7|note=[used]
memory:99|note=[not injected]
</citation_entries></oai-mem-citation>",
        1_234,
    );

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].memory_id, MemoryId(7));
    assert_eq!(events[0].kind, axon_memory::MemoryUsageKind::Cited);
    assert_eq!(events[0].occurred_at_ms, 1_234);
}

#[test]
fn hybrid_query_with_precomputed_embedding_reuses_embedding_report() {
    let report = QueryEmbeddingReport {
        embedding: Some(vec![0.1, 0.2, 0.3]),
        image_embedding: None,
        status: MEMORY_QUERY_EMBEDDING_GENERATED,
        reason: None,
    };
    let test_dir = axon_test_support::test_temp_root()
        .join(format!("axon-memory-read-test-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&test_dir);
    let memory_config =
        axon_memory::MemoryConfig::from_config_dir(test_dir.clone(), false, 1536).unwrap();
    let orchestrator = MemoryOrchestrator::init(memory_config).unwrap();
    let query =
        hybrid_query_memory_with_embedding(&orchestrator, "test query", &report, 5, 1_000, None);
    assert_eq!(query.embedding_status, MEMORY_QUERY_EMBEDDING_GENERATED);
    assert!(query.reason.is_none());
    let _ = std::fs::remove_dir_all(&test_dir);
}

fn turn_hit(memory_id: u64, content_ref: &str) -> MemoryTurnHit {
    MemoryTurnHit {
        memory_id,
        content_ref: content_ref.to_owned(),
    }
}
