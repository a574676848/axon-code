use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use clap::Args;
use serde::{Deserialize, Serialize};

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{MemoryCliContext, bounded_limit, display_path};

const MEMORY_SYNC_AUDIT_FILE: &str = "memory_sync_audit.jsonl";
const MEMORY_SYNC_AUDIT_STATUS_READY: &str = "memory_sync_audit_ready";

/// `axon memory sync-audit` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemorySyncAuditCommand {
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemorySyncAuditRecord {
    schema_version: u32,
    recorded_at_ms: u64,
    action: String,
    status: String,
    record_count: usize,
    records_imported: usize,
    conflict_count: usize,
    payload_digest_algorithm: String,
    payload_digest: String,
    signature_status: String,
    dry_run: bool,
    input_path_ref: Option<String>,
    output_path_ref: Option<String>,
    ref_only: bool,
    remote_write_allowed: bool,
    memory_summary_exported: bool,
    prompt_text_exported: bool,
    provider_raw_payload_exported: bool,
    reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemorySyncAuditReport {
    status: &'static str,
    audit_path: String,
    limit: usize,
    total_records: usize,
    records: Vec<MemorySyncAuditRecord>,
    raw_payload_exported: bool,
    remote_write_allowed: bool,
}

pub(super) fn run_memory_sync_audit(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemorySyncAuditCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let audit_path = memory_sync_audit_path(context);
    let records = match read_sync_audit_records(&audit_path) {
        Ok(records) => records,
        Err(error) => return cli_error(error),
    };
    let total_records = records.len();
    let records = records
        .into_iter()
        .rev()
        .take(limit)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let report = MemorySyncAuditReport {
        status: MEMORY_SYNC_AUDIT_STATUS_READY,
        audit_path: display_path(&audit_path),
        limit,
        total_records,
        records,
        raw_payload_exported: false,
        remote_write_allowed: false,
    };
    render_sync_audit(format, &report)
}

fn read_sync_audit_records(path: &Path) -> Result<Vec<MemorySyncAuditRecord>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).map_err(|_| "memory sync audit 读取失败".to_owned())?;
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str::<MemorySyncAuditRecord>(line)
                .map_err(|_| "memory sync audit JSON 无效".to_owned())
        })
        .collect()
}

fn memory_sync_audit_path(context: &MemoryCliContext) -> PathBuf {
    context
        .orchestrator
        .config()
        .memory_dir
        .join(MEMORY_SYNC_AUDIT_FILE)
}

fn render_sync_audit(format: OutputFormat, report: &MemorySyncAuditReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.records.is_empty() {
                stdout.push_str("没有 Memory sync audit 记录\n");
            } else {
                for record in &report.records {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\t{}\t{}\t{}",
                        record.recorded_at_ms,
                        record.action,
                        record.status,
                        record.record_count,
                        record.payload_digest
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
