use std::fmt::Write as _;

use axon_agent_runtime::{
    MemoryConflictListInput, MemoryConflictListReport, MemoryConsolidationRuntime,
};
use axon_domain::MemoryState;
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{MemoryCliContext, bounded_limit};

const MEMORY_CONFLICTS_STATUS_READY: &str = "memory_conflicts_ready";

/// `axon memory conflicts` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryConflictsCommand {
    /// 返回冲突组上限。
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConflictsReport {
    status: &'static str,
    conflict_group_count: usize,
    groups: Vec<MemoryConflictGroupView>,
    ref_only: bool,
    remote_write_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConflictGroupView {
    conflict_key: String,
    records: Vec<MemoryConflictRecordView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConflictRecordView {
    memory_id: u64,
    content_ref: Option<String>,
    namespace_ref: Option<String>,
    repo_ref: Option<String>,
    user_ref: Option<String>,
    status: &'static str,
    state: &'static str,
    recoverable_conflict: bool,
    updated_at_ms: u64,
    confidence_basis_points: u16,
    importance_basis_points: u16,
}

pub(super) fn run_memory_conflicts(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryConflictsCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let report = match MemoryConsolidationRuntime.list_conflicts(&MemoryConflictListInput {
        orchestrator: &context.orchestrator,
        limit,
    }) {
        Ok(report) => conflicts_report(&report),
        Err(error) => return cli_error(format!("{error:?}")),
    };
    render_conflicts(format, &report)
}

fn conflicts_report(report: &MemoryConflictListReport) -> MemoryConflictsReport {
    MemoryConflictsReport {
        status: MEMORY_CONFLICTS_STATUS_READY,
        conflict_group_count: report.conflict_group_count,
        groups: report
            .groups
            .iter()
            .map(|group| MemoryConflictGroupView {
                conflict_key: group.conflict_key.clone(),
                records: group
                    .records
                    .iter()
                    .map(|record| MemoryConflictRecordView {
                        memory_id: record.memory_id,
                        content_ref: record.content_ref.clone(),
                        namespace_ref: record.namespace_ref.clone(),
                        repo_ref: record.repo_ref.clone(),
                        user_ref: record.user_ref.clone(),
                        status: record.lifecycle_status(),
                        state: memory_state_code(record.state),
                        recoverable_conflict: record.recoverable_conflict,
                        updated_at_ms: record.updated_at_ms,
                        confidence_basis_points: record.confidence_basis_points,
                        importance_basis_points: record.importance_basis_points,
                    })
                    .collect(),
            })
            .collect(),
        ref_only: report.ref_only,
        remote_write_allowed: report.remote_write_allowed,
    }
}

fn render_conflicts(format: OutputFormat, report: &MemoryConflictsReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.groups.is_empty() {
                stdout.push_str("没有 Memory 冲突组\n");
            } else {
                for group in &report.groups {
                    let _ = writeln!(stdout, "{}", group.conflict_key);
                    for record in &group.records {
                        let _ = writeln!(
                            stdout,
                            "  {}\t{}\t{}\t{}",
                            record.memory_id,
                            record.status,
                            record.state,
                            record.content_ref.as_deref().unwrap_or("-")
                        );
                    }
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

const fn memory_state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}
