use std::fmt::Write as _;

use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat, display_units::format_duration_millis_compact,
    management_commands::json_result,
};

use super::view::MemoryRecordView;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryStatsReport {
    pub(super) status: &'static str,
    pub(super) config_path: String,
    pub(super) axon_root: String,
    pub(super) memory_dir: String,
    pub(super) retrieval_mode: &'static str,
    pub(super) index_health: MemoryIndexHealthView,
    pub(super) schema_version: i64,
    pub(super) total_records: usize,
    pub(super) active_records: usize,
    pub(super) archived_records: usize,
    pub(super) vector_records: usize,
    pub(super) memory_kind_counts: Vec<MemoryKindCountView>,
    pub(super) prompt_text_persisted: bool,
    pub(super) raw_payload_persisted: bool,
    pub(super) remote_write_allowed: bool,
    pub(super) consolidation: MemoryConsolidationStatsView,
    pub(super) artifacts: MemoryArtifactsStatsView,
    pub(super) pipeline: MemoryPipelineStatsView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryKindCountView {
    pub(super) memory_kind: &'static str,
    pub(super) count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryConsolidationStatsView {
    pub(super) interval_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryArtifactsStatsView {
    pub(super) raw_memories_present: bool,
    pub(super) rollout_summary_count: usize,
    pub(super) extension_count: usize,
    pub(super) phase2_diff_present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryPipelineStatsView {
    pub(super) explicit_write: usize,
    pub(super) phase1: usize,
    pub(super) phase2: usize,
    pub(super) needs_repair: usize,
    pub(super) failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryIndexHealthView {
    pub(super) text_index_available: bool,
    pub(super) vector_index_available: bool,
    pub(super) degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryDoctorReport {
    pub(super) status: &'static str,
    pub(super) memory_dir: String,
    pub(super) generated_at_ms: u64,
    pub(super) total_records: usize,
    pub(super) active_records: usize,
    pub(super) text_index_missing_records: usize,
    pub(super) vector_index_missing_records: usize,
    pub(super) unreadable_projection_count: usize,
    pub(super) repair_recommended: bool,
    pub(super) duplicate_cluster_count: usize,
    pub(super) duplicate_cluster_records: usize,
    pub(super) procedural_review_records: usize,
    pub(super) explicit_write_job_present: bool,
    pub(super) usage_store_ready: bool,
    pub(super) artifacts_ready: bool,
    pub(super) ref_only: bool,
    pub(super) remote_write_allowed: bool,
    pub(super) operator_action: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemoryListReport {
    pub(super) status: &'static str,
    pub(super) kind: Option<&'static str>,
    pub(super) memory_kind: Option<&'static str>,
    pub(super) limit: usize,
    pub(super) records: Vec<MemoryRecordView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemorySearchReport {
    pub(super) status: &'static str,
    pub(super) query: String,
    pub(super) limit: usize,
    pub(super) hits: Vec<MemorySearchHitView>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MemorySearchHitView {
    pub(super) score_basis_points: u16,
    pub(super) text_score: Option<f32>,
    pub(super) vector_distance: Option<f32>,
    pub(super) text_rank: Option<usize>,
    pub(super) vector_rank: Option<usize>,
    pub(super) record: MemoryRecordView,
}

pub(super) fn render_stats(format: OutputFormat, report: &MemoryStatsReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Axon Memory: {}\n- records: total={} active={} archived={} vector={}\n- retrieval: {} degraded={}\n- schema: {}\n- raw_flags: prompt={} payload={} remote={}\n- consolidation: interval={} source=axon_cron\n- artifacts: raw={} summaries={} extensions={} phase2_diff={}\n- pipeline: explicit_write={} phase1={} phase2={} needs_repair={} failed={}\n- path: {}\n",
                report.status,
                report.total_records,
                report.active_records,
                report.archived_records,
                report.vector_records,
                report.retrieval_mode,
                report.index_health.degraded,
                report.schema_version,
                report.prompt_text_persisted,
                report.raw_payload_persisted,
                report.remote_write_allowed,
                format_duration_millis_compact(report.consolidation.interval_ms),
                report.artifacts.raw_memories_present,
                report.artifacts.rollout_summary_count,
                report.artifacts.extension_count,
                report.artifacts.phase2_diff_present,
                report.pipeline.explicit_write,
                report.pipeline.phase1,
                report.pipeline.phase2,
                report.pipeline.needs_repair,
                report.pipeline.failed,
                report.memory_dir,
            ),
            stderr: String::new(),
        },
    }
}

pub(super) fn render_list(format: OutputFormat, report: &MemoryListReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.records.is_empty() {
                stdout.push_str("未找到 Memory 记录\n");
            } else {
                for record in &report.records {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\t{}\tconf={}\timportance={}",
                        record.memory_id,
                        record.state,
                        record.content_ref.as_deref().unwrap_or("-"),
                        record.confidence_basis_points,
                        record.importance_basis_points
                    );
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

pub(super) fn render_search(format: OutputFormat, report: &MemorySearchReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.hits.is_empty() {
                stdout.push_str("未命中 Memory 记录\n");
            } else {
                for hit in &report.hits {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\tscore={}\t{}",
                        hit.record.memory_id,
                        hit.record.state,
                        hit.score_basis_points,
                        hit.record.content_ref.as_deref().unwrap_or("-")
                    );
                }
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

pub(super) fn render_doctor(format: OutputFormat, report: &MemoryDoctorReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Memory doctor: status={} records={} active={} repair_recommended={} action={}\n- drift: text_missing={} vector_missing={} unreadable={}\n- governance: duplicate_clusters={} duplicate_records={} procedural_review={}\n- runtime: consolidation=axon_cron explicit_write_job={} usage_store={} artifacts={}\n",
                report.status,
                report.total_records,
                report.active_records,
                report.repair_recommended,
                report.operator_action,
                report.text_index_missing_records,
                report.vector_index_missing_records,
                report.unreadable_projection_count,
                report.duplicate_cluster_count,
                report.duplicate_cluster_records,
                report.procedural_review_records,
                report.explicit_write_job_present,
                report.usage_store_ready,
                report.artifacts_ready,
            ),
            stderr: String::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MemoryArtifactsStatsView, MemoryConsolidationStatsView, MemoryIndexHealthView,
        MemoryPipelineStatsView, MemoryStatsReport, render_stats,
    };
    use crate::OutputFormat;

    #[test]
    fn stats_text_scales_consolidation_interval() {
        let result = render_stats(
            OutputFormat::Text,
            &MemoryStatsReport {
                status: "ready",
                config_path: String::new(),
                axon_root: String::new(),
                memory_dir: String::from(".axon/memory"),
                retrieval_mode: "hybrid",
                schema_version: 1,
                total_records: 0,
                active_records: 0,
                archived_records: 0,
                vector_records: 0,
                memory_kind_counts: Vec::new(),
                index_health: MemoryIndexHealthView {
                    text_index_available: true,
                    vector_index_available: true,
                    degraded: false,
                },
                prompt_text_persisted: false,
                raw_payload_persisted: false,
                remote_write_allowed: false,
                consolidation: MemoryConsolidationStatsView {
                    interval_ms: 65_000,
                },
                artifacts: MemoryArtifactsStatsView {
                    raw_memories_present: false,
                    rollout_summary_count: 0,
                    extension_count: 0,
                    phase2_diff_present: false,
                },
                pipeline: MemoryPipelineStatsView {
                    explicit_write: 0,
                    phase1: 0,
                    phase2: 0,
                    needs_repair: 0,
                    failed: 0,
                },
            },
        );

        assert!(result.stdout.contains("consolidation: interval=1m 05s"));
        assert!(!result.stdout.contains("interval_ms=65000"));
    }
}
