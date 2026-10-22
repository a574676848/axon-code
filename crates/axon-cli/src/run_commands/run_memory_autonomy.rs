use std::path::Path;

use axon_agent_runtime::{
    MemoryAutonomyFeedbackInput, MemoryAutonomyFeedbackReport, MemoryAutonomyFeedbackRuntime,
};
use axon_config::AxonConfig;
use axon_domain::MemoryId;
use axon_memory_read::init_memory_orchestrator;

use super::{RunRequest, run_execution::now_ms, run_memory_usefulness::RunMemoryUsefulnessEvent};

pub(super) fn apply_memory_autonomy_feedback(
    config_path: &Path,
    config: &AxonConfig,
    request: &RunRequest,
    usefulness: Option<&RunMemoryUsefulnessEvent>,
    run_completed: bool,
) -> Option<MemoryAutonomyFeedbackReport> {
    let usefulness = usefulness?;
    let recall = request.memory_recall.as_ref()?;
    let orchestrator = init_memory_orchestrator(config_path, config).ok()?;
    let store = orchestrator.sqlite_store().ok()?;
    let memory_ids = recall
        .hit_memory_ids
        .iter()
        .copied()
        .map(MemoryId)
        .collect::<Vec<_>>();
    let records = store.records_by_memory_ids(&memory_ids).ok()?;
    let output = MemoryAutonomyFeedbackRuntime
        .apply(MemoryAutonomyFeedbackInput {
            records,
            adopted_hit_refs: usefulness.adopted_hit_refs.clone(),
            downrank_hit_refs: usefulness.downrank_recommended_hit_refs.clone(),
            scope_mismatch_hit_refs: recall
                .trace
                .iter()
                .filter(|hit| hit.scope_match == "scope_mismatch")
                .map(|hit| hit.content_ref.clone())
                .collect(),
            failed_tool_count: usefulness.failed_tool_count,
            run_completed,
            now_ms: now_ms(),
        })
        .ok()?;
    store.update_governance_records(&output.records).ok()?;
    Some(output.report)
}
