use std::{collections::BTreeMap, fs, time::Duration};

use axon_agent_runtime::{
    MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS, MEMORY_CONSOLIDATION_DEFAULT_INTERVAL_MS,
    MemoryPhysicalConsistencyCheckInput, MemoryPhysicalConsistencyCheckReport,
    MemoryPhysicalConsistencyCheckRuntime, MemoryRetrievalMode, MemorySqliteRecord,
    MemorySqliteRecordFilter,
};
use axon_config::ConfigLoader;
use axon_domain::{MemoryKind, MemorySourceOrigin, MemoryState};

use crate::memory_image_ref::memory_image_refs;
use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_config_error, cli_error},
};
use axon_memory::{
    MemoryArtifactsWorkspace, MemoryPipelineJobKind, MemoryPipelineStateStore, MemoryUsageStore,
    MemoryVisibility,
};
use axon_memory_read::{
    hybrid_query_memory_with_images, init_memory_orchestrator, injected_usage_events,
    record_read_usage,
};

use super::render::{
    MemoryArtifactsStatsView, MemoryConsolidationStatsView, MemoryDoctorReport,
    MemoryIndexHealthView, MemoryKindCountView, MemoryListReport, MemoryPipelineStatsView,
    MemorySearchHitView, MemorySearchReport, MemoryStatsReport, render_doctor, render_list,
    render_search, render_stats,
};
use super::view::record_view;
use super::{
    MEMORY_STATUS_READY, MemoryCliContext, MemoryDoctorCommand, MemoryKindFilter,
    MemoryListCommand, MemorySearchCommand, MemoryTypeFilter, bounded_limit, display_path, now_ms,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryDoctorGovernanceAudit {
    duplicate_cluster_count: usize,
    duplicate_cluster_records: usize,
    procedural_review_records: usize,
}

pub(super) fn run_memory_stats(format: OutputFormat, context: &MemoryCliContext) -> CliRunResult {
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let summary = match store.summary() {
        Ok(summary) => summary,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let schema_version = match store.schema_version() {
        Ok(version) => version,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let memory_kind_counts = match store.memory_kind_counts(MemoryVisibility::All, None) {
        Ok(counts) => counts
            .into_iter()
            .map(|count| MemoryKindCountView {
                memory_kind: count.memory_kind.as_str(),
                count: count.count,
            })
            .collect(),
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let report = MemoryStatsReport {
        status: MEMORY_STATUS_READY,
        config_path: display_path(&context.config_path),
        axon_root: display_path(&context.axon_root),
        memory_dir: display_path(&context.orchestrator.config().memory_dir),
        retrieval_mode: retrieval_mode_code(context.orchestrator.retrieval_mode()),
        index_health: memory_index_health(context),
        schema_version,
        total_records: summary.total_records,
        active_records: summary.active_records,
        archived_records: summary.archived_records,
        vector_records: summary.vector_records,
        memory_kind_counts,
        prompt_text_persisted: summary.prompt_text_persisted,
        raw_payload_persisted: summary.raw_payload_persisted,
        remote_write_allowed: summary.remote_write_allowed,
        consolidation: memory_consolidation_stats(),
        artifacts: memory_artifacts_stats(context),
        pipeline: memory_pipeline_stats(context),
    };
    render_stats(format, &report)
}

pub(super) fn run_memory_doctor(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryDoctorCommand,
) -> CliRunResult {
    let now_ms = now_ms();
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let summary = match store.summary() {
        Ok(summary) => summary,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let check =
        match MemoryPhysicalConsistencyCheckRuntime.check(&MemoryPhysicalConsistencyCheckInput {
            orchestrator: &context.orchestrator,
            now_ms,
            max_probe_records: command
                .max_probe_records
                .unwrap_or(MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS),
        }) {
            Ok(report) => report,
            Err(error) => return cli_error(format!("{error:?}")),
        };
    let unreadable_projection_count = usize::from(check.text_index_unreadable)
        + usize::from(check.vector_index_unreadable)
        + usize::from(context.orchestrator.index_health().degraded);
    let governance = memory_doctor_governance_audit(context);
    let explicit_write_job_present = memory_explicit_write_job_present(context);
    let usage_store_ready = memory_usage_store_ready(context);
    let artifacts_ready = memory_artifacts_ready(context);
    render_doctor(
        format,
        &MemoryDoctorReport {
            status: if check.repair_recommended {
                "memory_doctor_repair_required"
            } else {
                "memory_doctor_ready"
            },
            memory_dir: display_path(&context.orchestrator.config().memory_dir),
            generated_at_ms: now_ms,
            total_records: summary.total_records,
            active_records: summary.active_records,
            text_index_missing_records: check.text_index_missing_records,
            vector_index_missing_records: check.vector_index_missing_records,
            unreadable_projection_count,
            repair_recommended: check.repair_recommended,
            duplicate_cluster_count: governance.duplicate_cluster_count,
            duplicate_cluster_records: governance.duplicate_cluster_records,
            procedural_review_records: governance.procedural_review_records,
            explicit_write_job_present,
            usage_store_ready,
            artifacts_ready,
            ref_only: check.ref_only,
            remote_write_allowed: check.remote_write_allowed,
            operator_action: memory_doctor_action(&check),
        },
    )
}

fn memory_index_health(context: &MemoryCliContext) -> MemoryIndexHealthView {
    let health = context.orchestrator.index_health();
    MemoryIndexHealthView {
        text_index_available: health.text_index_available,
        vector_index_available: health.vector_index_available,
        degraded: health.degraded,
    }
}

pub(super) fn run_memory_list(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryListCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let records_result = match command.memory_kind {
        Some(memory_kind) => store.records_limited_by_kind_and_state(
            memory_type_code(memory_kind),
            command.kind.map(|kind| sqlite_record_filter(Some(kind))),
            limit,
        ),
        None => store.records_limited(sqlite_record_filter(command.kind), limit),
    };
    let records = match records_result {
        Ok(records) => records,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let records = records.iter().map(record_view).collect();
    let report = MemoryListReport {
        status: MEMORY_STATUS_READY,
        kind: command.kind.map(kind_filter_code),
        memory_kind: command.memory_kind.map(memory_type_code_str),
        limit,
        records,
    };
    render_list(format, &report)
}

pub(super) fn run_memory_search(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemorySearchCommand,
) -> CliRunResult {
    if command.query.trim().is_empty() {
        return cli_error("memory search 查询不能为空");
    }
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let image_refs = match memory_image_refs(&command.images) {
        Ok(image_refs) => image_refs,
        Err(error) => return cli_error(format!("memory search --image {error}")),
    };
    let loaded = match ConfigLoader::load_from_path(&context.config_path) {
        Ok(loaded) => loaded,
        Err(error) => return cli_config_error(&error),
    };
    let orchestrator = match init_memory_orchestrator(&context.config_path, &loaded.config) {
        Ok(orchestrator) => orchestrator,
        Err(error) => return cli_error(error),
    };
    let recall_now_ms = now_ms();
    let hits = hybrid_query_memory_with_images(
        &loaded.config,
        &orchestrator,
        &command.query,
        &image_refs,
        limit,
        recall_now_ms,
        Duration::from_secs(10),
        None,
    )
    .hits;
    let _ = record_read_usage(&orchestrator, &injected_usage_events(&hits, recall_now_ms));
    let report = MemorySearchReport {
        status: MEMORY_STATUS_READY,
        query: command.query.clone(),
        limit,
        hits: hits
            .into_iter()
            .map(|hit| MemorySearchHitView {
                score_basis_points: hit.score_basis_points,
                text_score: hit.text_score,
                vector_distance: hit.vector_distance,
                text_rank: hit.text_rank,
                vector_rank: hit.vector_rank,
                record: record_view(&hit.record),
            })
            .collect(),
    };
    render_search(format, &report)
}

fn memory_consolidation_stats() -> MemoryConsolidationStatsView {
    MemoryConsolidationStatsView {
        interval_ms: MEMORY_CONSOLIDATION_DEFAULT_INTERVAL_MS,
    }
}

fn memory_artifacts_stats(context: &MemoryCliContext) -> MemoryArtifactsStatsView {
    let workspace = MemoryArtifactsWorkspace::new(context.orchestrator.config());
    MemoryArtifactsStatsView {
        raw_memories_present: workspace.raw_memories_path().is_file(),
        rollout_summary_count: count_dir_entries(workspace.rollout_summaries_dir()),
        extension_count: count_dir_entries(workspace.extensions_dir()),
        phase2_diff_present: workspace.phase2_diff_path().is_file(),
    }
}

fn memory_pipeline_stats(context: &MemoryCliContext) -> MemoryPipelineStatsView {
    let Ok(store) = MemoryPipelineStateStore::open(context.orchestrator.config()) else {
        return MemoryPipelineStatsView {
            explicit_write: 0,
            phase1: 0,
            phase2: 0,
            needs_repair: 0,
            failed: 0,
        };
    };
    let explicit_write = store
        .stats_for_kind(MemoryPipelineJobKind::ExplicitWrite)
        .unwrap_or_default();
    let phase1 = store
        .stats_for_kind(MemoryPipelineJobKind::Phase1)
        .unwrap_or_default();
    let phase2 = store
        .stats_for_kind(MemoryPipelineJobKind::Phase2)
        .unwrap_or_default();
    MemoryPipelineStatsView {
        explicit_write: explicit_write.total,
        phase1: phase1.total,
        phase2: phase2.total,
        needs_repair: explicit_write
            .needs_repair
            .saturating_add(phase1.needs_repair)
            .saturating_add(phase2.needs_repair),
        failed: explicit_write
            .failed
            .saturating_add(phase1.failed)
            .saturating_add(phase2.failed),
    }
}

fn memory_explicit_write_job_present(context: &MemoryCliContext) -> bool {
    MemoryPipelineStateStore::open(context.orchestrator.config())
        .and_then(|store| store.has_kind(MemoryPipelineJobKind::ExplicitWrite))
        .unwrap_or(false)
}

fn memory_usage_store_ready(context: &MemoryCliContext) -> bool {
    MemoryUsageStore::open(context.orchestrator.config())
        .and_then(|store| store.initialize_schema())
        .is_ok()
}

fn memory_artifacts_ready(context: &MemoryCliContext) -> bool {
    MemoryArtifactsWorkspace::new(context.orchestrator.config())
        .ensure_layout()
        .is_ok()
}

fn count_dir_entries(path: impl AsRef<std::path::Path>) -> usize {
    fs::read_dir(path).map_or(0, |entries| entries.filter_map(Result::ok).count())
}

fn memory_doctor_governance_audit(context: &MemoryCliContext) -> MemoryDoctorGovernanceAudit {
    let records = context
        .orchestrator
        .sqlite_store()
        .and_then(|store| store.records())
        .unwrap_or_default();
    let mut clusters = BTreeMap::<String, usize>::new();
    let mut procedural_review_records = 0;
    for record in records.iter().filter(|record| {
        record.archived_at_ms.is_none()
            && matches!(record.record.state, MemoryState::Weak | MemoryState::Strong)
    }) {
        if let Some(key) = duplicate_cluster_key(record) {
            *clusters.entry(key).or_default() += 1;
        }
        if record
            .record
            .source_trace
            .as_ref()
            .is_some_and(|trace| trace.origin == MemorySourceOrigin::SkillReflection)
            && record.importance_basis_points < 6_000
        {
            procedural_review_records += 1;
        }
    }
    let duplicate_cluster_count = clusters.values().filter(|count| **count > 1).count();
    let duplicate_cluster_records = clusters.values().filter(|count| **count > 1).copied().sum();
    MemoryDoctorGovernanceAudit {
        duplicate_cluster_count,
        duplicate_cluster_records,
        procedural_review_records,
    }
}

fn duplicate_cluster_key(record: &MemorySqliteRecord) -> Option<String> {
    let source = record.record.source_trace.as_ref()?;
    let evidence = source
        .evidence_ref
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| (!source.summary.trim().is_empty()).then_some(source.summary.as_str()))?;
    Some(format!(
        "{}:{}:{:?}:{evidence}",
        record
            .namespace_ref
            .as_deref()
            .unwrap_or("memory-scope:any"),
        record.repo_ref.as_deref().unwrap_or("repo-scope:any"),
        source.origin
    ))
}

fn memory_doctor_action(check: &MemoryPhysicalConsistencyCheckReport) -> &'static str {
    if check.repair_recommended {
        "run_memory_consistency_repair"
    } else {
        "none"
    }
}

const fn sqlite_record_filter(filter: Option<MemoryKindFilter>) -> MemorySqliteRecordFilter {
    match filter {
        None => MemorySqliteRecordFilter::All,
        Some(MemoryKindFilter::Active) => MemorySqliteRecordFilter::Active,
        Some(MemoryKindFilter::Archived) => MemorySqliteRecordFilter::Archived,
        Some(MemoryKindFilter::Strong) => MemorySqliteRecordFilter::Strong,
        Some(MemoryKindFilter::Weak) => MemorySqliteRecordFilter::Weak,
        Some(MemoryKindFilter::Quarantined) => MemorySqliteRecordFilter::Quarantined,
        Some(MemoryKindFilter::Tombstone) => MemorySqliteRecordFilter::Tombstone,
    }
}

const fn retrieval_mode_code(mode: MemoryRetrievalMode) -> &'static str {
    match mode {
        MemoryRetrievalMode::Hybrid => "hybrid",
        MemoryRetrievalMode::TextFallback => "text_fallback",
        MemoryRetrievalMode::CoreFallback => "core_fallback",
    }
}

const fn kind_filter_code(kind: MemoryKindFilter) -> &'static str {
    match kind {
        MemoryKindFilter::Active => "active",
        MemoryKindFilter::Archived => "archived",
        MemoryKindFilter::Strong => "strong",
        MemoryKindFilter::Weak => "weak",
        MemoryKindFilter::Quarantined => "quarantined",
        MemoryKindFilter::Tombstone => "tombstone",
    }
}

const fn memory_type_code(filter: MemoryTypeFilter) -> MemoryKind {
    match filter {
        MemoryTypeFilter::Profile => MemoryKind::Profile,
        MemoryTypeFilter::Preference => MemoryKind::Preference,
        MemoryTypeFilter::Experience => MemoryKind::Experience,
        MemoryTypeFilter::Knowledge => MemoryKind::Knowledge,
        MemoryTypeFilter::Procedure => MemoryKind::Procedure,
    }
}

const fn memory_type_code_str(filter: MemoryTypeFilter) -> &'static str {
    memory_type_code(filter).as_str()
}
