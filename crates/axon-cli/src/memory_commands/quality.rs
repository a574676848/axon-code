use std::{fmt::Write as _, fs, path::PathBuf};

use axon_agent_runtime::{
    MemoryGoldenCase, MemoryGoldenCaseHits, MemoryGoldenHit, MemoryGoldenSetEvaluationInput,
    MemoryGoldenSetEvaluationReport, MemoryHybridQuery, MemoryQualityEvaluationInput,
    MemoryQualityEvaluationReport, MemoryQualityEvaluationRuntime,
};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{MemoryCliContext, display_path, now_ms};

const MEMORY_QUALITY_STATUS_READY: &str = "memory_quality_ready";
const MEMORY_QUALITY_DEFAULT_GOLDEN_SET_PATH: &str = "docs/memory-golden-set.default.json";

/// `axon memory quality` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryQualityCommand {
    /// Golden set JSON 文件路径。
    #[arg(long)]
    pub golden_set: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryQualityReport {
    status: &'static str,
    store_path: String,
    generated_at_ms: u64,
    total_records: usize,
    active_records: usize,
    pollution_records: usize,
    stale_fact_residual_records: usize,
    false_recall_risk_records: usize,
    duplicate_active_residual_records: usize,
    pollution_basis_points: u16,
    stale_fact_basis_points: u16,
    false_recall_basis_points: u16,
    duplicate_active_residual_basis_points: u16,
    runtime: MemoryRuntimeQualityView,
    ref_only: bool,
    quality_gate_passed: bool,
    reason: &'static str,
    golden: Option<MemoryGoldenSetReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryRuntimeQualityView {
    observation_count: u64,
    cache_read_input_tokens: u64,
    complete_input_tokens: u64,
    cache_hit_basis_points: u16,
    reflection_attempts: u64,
    reflection_yields: u64,
    reflection_yield_basis_points: u16,
    semantic_assessments: u64,
    false_memory_rejections: u64,
    false_memory_rejection_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryGoldenSetReport {
    total_cases: usize,
    expected_hits: usize,
    forbidden_hits: usize,
    missing_cases: usize,
    pass_basis_points: u16,
    avoidance_hits: usize,
    avoidance_basis_points: u16,
    recall_usefulness: MemoryRecallUsefulnessReport,
    golden_gate_passed: bool,
    reason: &'static str,
    case_results: Vec<MemoryGoldenCaseResultView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryRecallUsefulnessReport {
    cited_hit_count: usize,
    used_hit_basis_points: u16,
    tool_reduction_proxy_basis_points: u16,
    repeated_error_avoidance_basis_points: u16,
    false_recall_risk_basis_points: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryGoldenCaseResultView {
    case_id: String,
    query_text: String,
    expected_content_ref: String,
    expected_hit: bool,
    forbidden_hit: bool,
    expected_rank: Option<usize>,
    top_content_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryGoldenSetFile {
    cases: Vec<MemoryGoldenCaseFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryGoldenCaseFile {
    case_id: String,
    query_text: String,
    expected_content_ref: String,
    #[serde(default)]
    forbidden_content_refs: Vec<String>,
}

pub(super) fn run_memory_quality(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryQualityCommand,
) -> CliRunResult {
    let store_path = &context.orchestrator.config().db_path;
    let now_ms = now_ms();
    let report = match MemoryQualityEvaluationRuntime.evaluate(&MemoryQualityEvaluationInput {
        orchestrator: &context.orchestrator,
        now_ms,
    }) {
        Ok(report) => quality_report(store_path, now_ms, &report),
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let report = match attach_golden_report(context, command, now_ms, report) {
        Ok(report) => report,
        Err(error) => return cli_error(error),
    };
    render_quality(format, &report)
}

fn quality_report(
    store_path: &std::path::Path,
    generated_at_ms: u64,
    report: &MemoryQualityEvaluationReport,
) -> MemoryQualityReport {
    MemoryQualityReport {
        status: MEMORY_QUALITY_STATUS_READY,
        store_path: display_path(store_path),
        generated_at_ms,
        total_records: report.total_records,
        active_records: report.active_records,
        pollution_records: report.pollution_records,
        stale_fact_residual_records: report.stale_fact_residual_records,
        false_recall_risk_records: report.false_recall_risk_records,
        duplicate_active_residual_records: report.duplicate_active_residual_records,
        pollution_basis_points: report.pollution_basis_points,
        stale_fact_basis_points: report.stale_fact_basis_points,
        false_recall_basis_points: report.false_recall_basis_points,
        duplicate_active_residual_basis_points: report.duplicate_active_residual_basis_points,
        runtime: MemoryRuntimeQualityView {
            observation_count: report.runtime.observation_count,
            cache_read_input_tokens: report.runtime.cache_read_input_tokens,
            complete_input_tokens: report.runtime.complete_input_tokens,
            cache_hit_basis_points: report.runtime.cache_hit_basis_points,
            reflection_attempts: report.runtime.reflection_attempts,
            reflection_yields: report.runtime.reflection_yields,
            reflection_yield_basis_points: report.runtime.reflection_yield_basis_points,
            semantic_assessments: report.runtime.semantic_assessments,
            false_memory_rejections: report.runtime.false_memory_rejections,
            false_memory_rejection_basis_points: report.runtime.false_memory_rejection_basis_points,
        },
        ref_only: report.ref_only,
        quality_gate_passed: report.quality_gate_passed,
        reason: report.reason,
        golden: None,
    }
}

fn attach_golden_report(
    context: &MemoryCliContext,
    command: &MemoryQualityCommand,
    now_ms: u64,
    mut report: MemoryQualityReport,
) -> Result<MemoryQualityReport, String> {
    let Some(golden_set_path) = resolve_golden_set_path(command) else {
        return Ok(report);
    };
    let golden_file = read_golden_set_file(&golden_set_path)?;
    let cases = golden_file
        .cases
        .into_iter()
        .map(|case| MemoryGoldenCase {
            case_id: case.case_id,
            query_text: case.query_text,
            expected_content_ref: case.expected_content_ref,
            forbidden_content_refs: case.forbidden_content_refs,
        })
        .collect::<Vec<_>>();
    let hits_by_case = cases
        .iter()
        .map(|case| golden_case_hits(context, case, now_ms))
        .collect::<Result<Vec<_>, _>>()?;
    let golden_report = MemoryQualityEvaluationRuntime
        .evaluate_golden_set(&MemoryGoldenSetEvaluationInput {
            cases,
            hits_by_case,
        })
        .map_err(|error| format!("{error:?}"))?;
    report.golden = Some(golden_set_report(&golden_report));
    Ok(report)
}

fn resolve_golden_set_path(command: &MemoryQualityCommand) -> Option<PathBuf> {
    if let Some(path) = command.golden_set.as_ref() {
        return Some(path.clone());
    }
    let default_path = PathBuf::from(MEMORY_QUALITY_DEFAULT_GOLDEN_SET_PATH);
    if default_path.exists() {
        return Some(default_path);
    }
    let workspace_default_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(MEMORY_QUALITY_DEFAULT_GOLDEN_SET_PATH);
    workspace_default_path
        .exists()
        .then_some(workspace_default_path)
}

fn read_golden_set_file(path: &std::path::Path) -> Result<MemoryGoldenSetFile, String> {
    let text =
        fs::read_to_string(path).map_err(|_| "无法读取 Memory golden set 文件".to_owned())?;
    serde_json::from_str(&text).map_err(|_| "Memory golden set JSON 无效".to_owned())
}

fn golden_case_hits(
    context: &MemoryCliContext,
    case: &MemoryGoldenCase,
    now_ms: u64,
) -> Result<MemoryGoldenCaseHits, String> {
    let recallable_records = context
        .orchestrator
        .sqlite_store()
        .and_then(|store| store.recallable_record_count(now_ms))
        .map_err(|error| format!("{error:?}"))?;
    let hits = if recallable_records == 0 {
        Vec::new()
    } else {
        context
            .orchestrator
            .hybrid_search(&MemoryHybridQuery {
                query_text: case.query_text.clone(),
                query_embedding: None,
                limit: recallable_records,
                now_ms,
                scope: None,
            })
            .map_err(|error| format!("{error:?}"))?
            .into_iter()
            .enumerate()
            .filter_map(|(index, hit)| {
                hit.record
                    .record
                    .content_ref
                    .map(|content_ref| MemoryGoldenHit {
                        content_ref,
                        rank: index + 1,
                        score_basis_points: hit.score_basis_points,
                    })
            })
            .collect()
    };
    Ok(MemoryGoldenCaseHits {
        case_id: case.case_id.clone(),
        hits,
    })
}

fn golden_set_report(report: &MemoryGoldenSetEvaluationReport) -> MemoryGoldenSetReport {
    MemoryGoldenSetReport {
        total_cases: report.total_cases,
        expected_hits: report.expected_hits,
        forbidden_hits: report.forbidden_hits,
        missing_cases: report.missing_cases,
        pass_basis_points: report.pass_basis_points,
        avoidance_hits: report.avoidance_hits,
        avoidance_basis_points: report.avoidance_basis_points,
        recall_usefulness: recall_usefulness_report(report),
        golden_gate_passed: report.golden_gate_passed,
        reason: report.reason,
        case_results: report
            .case_results
            .iter()
            .map(|case| MemoryGoldenCaseResultView {
                case_id: case.case_id.clone(),
                query_text: case.query_text.clone(),
                expected_content_ref: case.expected_content_ref.clone(),
                expected_hit: case.expected_hit,
                forbidden_hit: case.forbidden_hit,
                expected_rank: case.expected_rank,
                top_content_ref: case.top_content_ref.clone(),
            })
            .collect(),
    }
}

fn recall_usefulness_report(
    report: &MemoryGoldenSetEvaluationReport,
) -> MemoryRecallUsefulnessReport {
    let false_recall_risk_basis_points = if report.total_cases == 0 {
        0
    } else {
        basis_points(report.forbidden_hits, report.total_cases)
    };
    MemoryRecallUsefulnessReport {
        cited_hit_count: report.expected_hits,
        used_hit_basis_points: report.pass_basis_points,
        tool_reduction_proxy_basis_points: report.pass_basis_points,
        repeated_error_avoidance_basis_points: report.avoidance_basis_points,
        false_recall_risk_basis_points,
    }
}

fn basis_points(numerator: usize, denominator: usize) -> u16 {
    let value = (numerator.saturating_mul(10_000) / denominator).min(10_000);
    u16::try_from(value).unwrap_or(10_000)
}

fn render_quality(format: OutputFormat, report: &MemoryQualityReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "Memory quality: passed={} records={} pollution={} stale={} falseRecall={} duplicateActive={} active={}",
                report.quality_gate_passed,
                report.total_records,
                report.pollution_basis_points,
                report.stale_fact_basis_points,
                report.false_recall_basis_points,
                report.duplicate_active_residual_basis_points,
                report.active_records
            );
            let _ = writeln!(
                stdout,
                "Memory runtime quality: cacheHit={} reflectionYield={} falseMemoryRejection={} observations={}",
                report.runtime.cache_hit_basis_points,
                report.runtime.reflection_yield_basis_points,
                report.runtime.false_memory_rejection_basis_points,
                report.runtime.observation_count
            );
            if let Some(golden) = report.golden.as_ref() {
                let _ = writeln!(
                    stdout,
                    "Memory golden set: passed={} pass={} avoidance={} cases={}",
                    golden.golden_gate_passed,
                    golden.pass_basis_points,
                    golden.avoidance_basis_points,
                    golden.total_cases
                );
            }
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}
