use std::fmt::Write as _;

use axon_agent_runtime::{
    MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS, MemoryPhysicalConsistencyCheckInput,
    MemoryPhysicalConsistencyCheckReport, MemoryPhysicalConsistencyCheckRuntime,
    MemoryPhysicalConsistencyRepairInput, MemoryPhysicalConsistencyRepairReport,
    MemoryPhysicalConsistencyRepairRuntime,
};
use clap::Args;
use serde::Serialize;

use crate::{MemoryOpsOutputFormat, MemoryOpsResult, cli_error, json_result};

use super::{MemoryOpsContext, display_path, now_ms};

const MEMORY_CONSISTENCY_STATUS_READY: &str = "memory_consistency_ready";
const MEMORY_CONSISTENCY_STATUS_REPAIRED: &str = "memory_consistency_repaired";
const MEMORY_CONSISTENCY_ACTION_NONE: &str = "none";
const MEMORY_CONSISTENCY_ACTION_REPAIR: &str = "run_memory_consistency_repair";

/// `axon memory consistency` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryConsistencyCommand {
    /// 最大物理投影探测记录数；省略时使用 runtime 默认值。
    #[arg(long)]
    pub max_probe_records: Option<usize>,
    /// 发现可重建投影缺口时执行修复。
    #[arg(long)]
    pub repair: bool,
    /// 最大修复记录数；省略时使用 runtime 默认值。
    #[arg(long)]
    pub max_repair_records: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConsistencyReport {
    status: &'static str,
    memory_dir: String,
    generated_at_ms: u64,
    check: MemoryConsistencyCheckView,
    repair: Option<MemoryConsistencyRepairView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConsistencyCheckView {
    sqlite_recallable_records: usize,
    checked_records: usize,
    probe_budget_records: usize,
    probe_truncated: bool,
    text_index_missing_records: usize,
    text_index_unreadable: bool,
    vector_records_deferred: usize,
    vector_index_unreadable: bool,
    vector_index_missing_records: usize,
    vector_index_records: usize,
    orphan_projection_records: usize,
    orphan_text_documents: usize,
    repair_recommended: bool,
    operator_action: &'static str,
    ref_only: bool,
    remote_write_allowed: bool,
    reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryConsistencyRepairView {
    sqlite_recallable_records: usize,
    text_index_rebuilt: usize,
    vector_index_rebuilt: usize,
    vector_records_deferred: usize,
    orphan_projections_pruned: usize,
    orphan_text_documents_pruned: usize,
    repair_truncated: bool,
    ref_only: bool,
    remote_write_allowed: bool,
    reason: &'static str,
}

/// 执行 Memory 物理投影一致性检查与可选修复。
#[must_use]
pub fn run_memory_consistency(
    format: MemoryOpsOutputFormat,
    context: &MemoryOpsContext,
    command: &MemoryConsistencyCommand,
) -> MemoryOpsResult {
    let now_ms = now_ms();
    let check_input = MemoryPhysicalConsistencyCheckInput {
        orchestrator: &context.orchestrator,
        now_ms,
        max_probe_records: command
            .max_probe_records
            .unwrap_or(MEMORY_CONSISTENCY_DEFAULT_PROBE_RECORDS),
    };
    let before = match MemoryPhysicalConsistencyCheckRuntime.check(&check_input) {
        Ok(report) => report,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let repair = if command.repair && before.repair_recommended {
        match MemoryPhysicalConsistencyRepairRuntime.repair(&MemoryPhysicalConsistencyRepairInput {
            orchestrator: &context.orchestrator,
            now_ms,
            max_repair_records: command.max_repair_records.unwrap_or_default(),
        }) {
            Ok(report) => Some(repair_view(&report)),
            Err(error) => return cli_error(format!("{error:?}")),
        }
    } else {
        None
    };
    let status = if repair.is_some() {
        MEMORY_CONSISTENCY_STATUS_REPAIRED
    } else {
        MEMORY_CONSISTENCY_STATUS_READY
    };
    let check = if repair.is_some() {
        match MemoryPhysicalConsistencyCheckRuntime.check(&check_input) {
            Ok(report) => check_view(&report),
            Err(error) => return cli_error(format!("{error:?}")),
        }
    } else {
        check_view(&before)
    };
    render_consistency(
        format,
        &MemoryConsistencyReport {
            status,
            memory_dir: display_path(&context.orchestrator.config().memory_dir),
            generated_at_ms: now_ms,
            check,
            repair,
        },
    )
}

fn check_view(report: &MemoryPhysicalConsistencyCheckReport) -> MemoryConsistencyCheckView {
    MemoryConsistencyCheckView {
        sqlite_recallable_records: report.sqlite_recallable_records,
        checked_records: report.checked_records,
        probe_budget_records: report.probe_budget_records,
        probe_truncated: report.probe_truncated,
        text_index_missing_records: report.text_index_missing_records,
        text_index_unreadable: report.text_index_unreadable,
        vector_records_deferred: report.vector_records_deferred,
        vector_index_unreadable: report.vector_index_unreadable,
        vector_index_missing_records: report.vector_index_missing_records,
        vector_index_records: report.vector_index_records,
        orphan_projection_records: report.orphan_projection_records,
        orphan_text_documents: report.orphan_text_documents,
        repair_recommended: report.repair_recommended,
        operator_action: operator_action(report),
        ref_only: report.ref_only,
        remote_write_allowed: report.remote_write_allowed,
        reason: report.reason,
    }
}

fn repair_view(report: &MemoryPhysicalConsistencyRepairReport) -> MemoryConsistencyRepairView {
    MemoryConsistencyRepairView {
        sqlite_recallable_records: report.sqlite_recallable_records,
        text_index_rebuilt: report.text_index_rebuilt,
        vector_index_rebuilt: report.vector_index_rebuilt,
        vector_records_deferred: report.vector_records_deferred,
        orphan_projections_pruned: report.orphan_projections_pruned,
        orphan_text_documents_pruned: report.orphan_text_documents_pruned,
        repair_truncated: report.repair_truncated,
        ref_only: report.ref_only,
        remote_write_allowed: report.remote_write_allowed,
        reason: report.reason,
    }
}

fn render_consistency(
    format: MemoryOpsOutputFormat,
    report: &MemoryConsistencyReport,
) -> MemoryOpsResult {
    match format {
        MemoryOpsOutputFormat::Json => json_result(report),
        MemoryOpsOutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "Memory consistency: status={} checked={}/{} repair_recommended={} action={}",
                report.status,
                report.check.checked_records,
                report.check.probe_budget_records,
                report.check.repair_recommended,
                report.check.operator_action
            );
            let _ = writeln!(
                stdout,
                "- drift: text_unreadable={} text_missing={} vector_unreadable={} vector_missing={} vector_deferred={}",
                report.check.text_index_unreadable,
                report.check.text_index_missing_records,
                report.check.vector_index_unreadable,
                report.check.vector_index_missing_records,
                report.check.vector_records_deferred
            );
            let _ = writeln!(
                stdout,
                "- orphan: projections={} text_documents={}",
                report.check.orphan_projection_records, report.check.orphan_text_documents
            );
            if let Some(repair) = &report.repair {
                let _ = writeln!(
                    stdout,
                    "- repaired: text={} vector={} vector_deferred={} truncated={}",
                    repair.text_index_rebuilt,
                    repair.vector_index_rebuilt,
                    repair.vector_records_deferred,
                    repair.repair_truncated
                );
                let _ = writeln!(
                    stdout,
                    "- pruned: projections={} text_documents={}",
                    repair.orphan_projections_pruned, repair.orphan_text_documents_pruned
                );
            }
            MemoryOpsResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

fn operator_action(report: &MemoryPhysicalConsistencyCheckReport) -> &'static str {
    if report.repair_recommended {
        MEMORY_CONSISTENCY_ACTION_REPAIR
    } else {
        MEMORY_CONSISTENCY_ACTION_NONE
    }
}
