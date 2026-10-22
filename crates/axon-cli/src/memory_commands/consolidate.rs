use std::fmt::Write as _;

use axon_agent_runtime::{
    MemoryConsolidationExplanation, MemoryConsolidationInput, MemoryConsolidationReport,
    MemoryConsolidationRuntime, MemorySqliteRecord,
};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{MemoryCliContext, display_path, now_ms};

const MEMORY_CONSOLIDATE_STATUS_READY: &str = "memory_consolidation_ready";
const MEMORY_CONSOLIDATE_STATUS_DRY_RUN: &str = "memory_consolidation_dry_run";

/// `axon memory consolidate` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryConsolidateCommand {
    /// 只预览当前 store 状态，不写入治理变更。
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConsolidateReport {
    status: &'static str,
    database_path: String,
    generated_at_ms: u64,
    ttl_cleaned: usize,
    low_activity_archived: usize,
    duplicate_tombstones: usize,
    conflict_archived: usize,
    projection_pruned_records: usize,
    active_records: usize,
    ref_only: bool,
    remote_write_allowed: bool,
    reason: &'static str,
    explanations: Vec<MemoryConsolidationExplanationView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConsolidationExplanationView {
    action: &'static str,
    affected_records: usize,
    summary: &'static str,
}

pub(super) fn run_memory_consolidate(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryConsolidateCommand,
) -> CliRunResult {
    let now_ms = now_ms();
    let report = if command.dry_run {
        match dry_run_report(context, now_ms) {
            Ok(report) => report,
            Err(error) => return cli_error(format!("{error:?}")),
        }
    } else {
        match MemoryConsolidationRuntime.consolidate(&MemoryConsolidationInput {
            orchestrator: &context.orchestrator,
            now_ms,
        }) {
            Ok(report) => consolidation_report(context, now_ms, &report),
            Err(error) => return cli_error(format!("{error:?}")),
        }
    };
    render_consolidate(format, &report)
}

fn dry_run_report(
    context: &MemoryCliContext,
    generated_at_ms: u64,
) -> axon_domain::AxonResult<MemoryConsolidateReport> {
    let records = context.orchestrator.sqlite_store()?.records()?;
    Ok(MemoryConsolidateReport {
        status: MEMORY_CONSOLIDATE_STATUS_DRY_RUN,
        database_path: display_path(&context.orchestrator.config().db_path),
        generated_at_ms,
        ttl_cleaned: expired_records(&records, generated_at_ms),
        low_activity_archived: 0,
        duplicate_tombstones: 0,
        conflict_archived: 0,
        projection_pruned_records: records
            .iter()
            .filter(|record| record.archived_at_ms.is_some())
            .count(),
        active_records: records
            .iter()
            .filter(|record| record.archived_at_ms.is_none())
            .count(),
        ref_only: records
            .iter()
            .all(|record| !record.prompt_text_persisted && !record.raw_payload_persisted),
        remote_write_allowed: records.iter().any(|record| record.remote_write_allowed),
        reason: "memory consolidation dry run",
        explanations: Vec::new(),
    })
}

fn expired_records(records: &[MemorySqliteRecord], now_ms: u64) -> usize {
    records
        .iter()
        .filter(|record| {
            record
                .ttl_expires_at_ms
                .is_some_and(|expires_at_ms| expires_at_ms <= now_ms)
        })
        .count()
}

fn consolidation_report(
    context: &MemoryCliContext,
    generated_at_ms: u64,
    report: &MemoryConsolidationReport,
) -> MemoryConsolidateReport {
    MemoryConsolidateReport {
        status: MEMORY_CONSOLIDATE_STATUS_READY,
        database_path: display_path(&context.orchestrator.config().db_path),
        generated_at_ms,
        ttl_cleaned: report.ttl_cleaned,
        low_activity_archived: report.low_activity_archived,
        duplicate_tombstones: report.duplicate_tombstones,
        conflict_archived: report.conflict_archived,
        projection_pruned_records: report.projection_pruned_records,
        active_records: report.active_records,
        ref_only: report.ref_only,
        remote_write_allowed: report.remote_write_allowed,
        reason: report.reason,
        explanations: report.explanations.iter().map(explanation_view).collect(),
    }
}

const fn explanation_view(
    explanation: &MemoryConsolidationExplanation,
) -> MemoryConsolidationExplanationView {
    MemoryConsolidationExplanationView {
        action: explanation.action,
        affected_records: explanation.affected_records,
        summary: explanation.summary,
    }
}

fn render_consolidate(format: OutputFormat, report: &MemoryConsolidateReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "Memory consolidation: ttl={} archived={} duplicates={} conflicts={} projection_pruned={} active={}",
                report.ttl_cleaned,
                report.low_activity_archived,
                report.duplicate_tombstones,
                report.conflict_archived,
                report.projection_pruned_records,
                report.active_records
            );
            for explanation in &report.explanations {
                let _ = writeln!(
                    stdout,
                    "- {}: {} record(s) - {}",
                    explanation.action, explanation.affected_records, explanation.summary
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
