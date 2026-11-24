use std::fmt::Write as _;

use axon_agent_runtime::{
    MemoryConflictRecoveryInput, MemoryConflictRecoveryReport, MemoryConflictRecoveryRuntime,
};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{MemoryCliContext, display_path, now_ms};

const MEMORY_RECOVER_CONFLICT_STATUS_READY: &str = "memory_conflict_recovered";

/// `axon memory recover-conflict` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryRecoverConflictCommand {
    /// 目标 memory id；多个 ID 必须同时传入 --yes。
    pub memory_ids: Vec<u64>,
    /// 确认批量恢复。
    #[arg(long)]
    pub yes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryRecoverConflictReport {
    status: &'static str,
    store_path: String,
    generated_at_ms: u64,
    memory_id: Option<u64>,
    recovered: Option<bool>,
    previous_status: Option<&'static str>,
    current_status: Option<&'static str>,
    recovered_count: usize,
    items: Vec<MemoryRecoverConflictItemView>,
    ref_only: bool,
    remote_write_allowed: bool,
    reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryRecoverConflictItemView {
    memory_id: u64,
    recovered: bool,
    previous_status: &'static str,
    current_status: &'static str,
}

pub(super) fn run_memory_recover_conflict(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryRecoverConflictCommand,
) -> CliRunResult {
    let store_path = &context.orchestrator.config().db_path;
    let now_ms = now_ms();
    let report = match MemoryConflictRecoveryRuntime.recover(&MemoryConflictRecoveryInput {
        orchestrator: &context.orchestrator,
        memory_ids: &command.memory_ids,
        now_ms,
        batch_approved: command.yes,
    }) {
        Ok(report) => recover_report(store_path, now_ms, &report),
        Err(error) => return cli_error(format!("{error:?}")),
    };
    render_recover(format, &report)
}

fn recover_report(
    store_path: &std::path::Path,
    generated_at_ms: u64,
    report: &MemoryConflictRecoveryReport,
) -> MemoryRecoverConflictReport {
    let single = (report.items.len() == 1).then(|| &report.items[0]);
    MemoryRecoverConflictReport {
        status: MEMORY_RECOVER_CONFLICT_STATUS_READY,
        store_path: display_path(store_path),
        generated_at_ms,
        memory_id: single.map(|item| item.memory_id),
        recovered: single.map(|item| item.recovered),
        previous_status: single.map(|item| lifecycle_status(item.previous_archived)),
        current_status: single.map(|item| lifecycle_status(item.current_archived)),
        recovered_count: report.items.len(),
        items: report.items.iter().map(recover_item).collect(),
        ref_only: report.ref_only,
        remote_write_allowed: report.remote_write_allowed,
        reason: report.reason,
    }
}

fn recover_item(
    item: &axon_agent_runtime::MemoryConflictRecoveryItem,
) -> MemoryRecoverConflictItemView {
    MemoryRecoverConflictItemView {
        memory_id: item.memory_id,
        recovered: item.recovered,
        previous_status: lifecycle_status(item.previous_archived),
        current_status: lifecycle_status(item.current_archived),
    }
}

fn render_recover(format: OutputFormat, report: &MemoryRecoverConflictReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "Memory conflict recovered: count={}",
                report.recovered_count
            );
            for item in &report.items {
                let _ = writeln!(
                    stdout,
                    "  id={} {} -> {}",
                    item.memory_id, item.previous_status, item.current_status
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

const fn lifecycle_status(archived: bool) -> &'static str {
    if archived { "archived" } else { "active" }
}
