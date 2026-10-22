use std::path::Path;

use axon_config::AxonConfig;
use axon_memory_read::{
    MemoryTurnContext, init_memory_orchestrator, memory_turn_cited_usage_events, record_read_usage,
};

use super::{RunRequest, run_execution::now_ms};

/// 记录模型输出中被显式 citation 的 Memory usage。
pub(super) fn record_cited_memory_usage(
    config_path: &Path,
    config: &AxonConfig,
    request: &RunRequest,
    output_text: &str,
) {
    if output_text.trim().is_empty() {
        return;
    }
    let Some(recall) = request.memory_recall.as_ref() else {
        return;
    };
    let context = MemoryTurnContext {
        surface: "cli_run".to_owned(),
        recalled_hits: recall.memory_turn_hits.clone(),
        injected_hits: recall.memory_turn_hits.clone(),
    };
    let events = memory_turn_cited_usage_events(&context, output_text, now_ms());
    if events.is_empty() {
        return;
    }
    let Ok(orchestrator) = init_memory_orchestrator(config_path, config) else {
        return;
    };
    let _ = record_read_usage(&orchestrator, &events);
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_memory_read::memory_turn_citation_report;

    #[test]
    fn citation_usage_filters_unrecalled_memory_ids() {
        let context = MemoryTurnContext {
            surface: "test".to_owned(),
            recalled_hits: vec![axon_memory_read::MemoryTurnHit {
                memory_id: 7,
                content_ref: "memory-ref:used".to_owned(),
            }],
            injected_hits: vec![axon_memory_read::MemoryTurnHit {
                memory_id: 7,
                content_ref: "memory-ref:used".to_owned(),
            }],
        };
        let report = memory_turn_citation_report(
            &context,
            "<oai-mem-citation><citation_entries>
memory:7|note=[used]
memory:99|note=[not recalled]
</citation_entries></oai-mem-citation>",
            "",
        );

        assert_eq!(report.cited_memory_ids, [7]);
        assert_eq!(report.cited_hit_refs, ["memory-ref:used"]);
    }
}
