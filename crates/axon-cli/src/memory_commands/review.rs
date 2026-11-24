use std::fmt::Write as _;

use axon_agent_runtime::MemorySqliteRecord;
use axon_domain::{MemorySourceOrigin, MemoryState};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{
    MemoryCliContext, bounded_limit,
    view::{MemoryRecordView, record_view},
};

const MEMORY_REVIEW_STATUS_READY: &str = "memory_review_ready";
const REVIEW_REASON_WEAK: &str = "weak_memory_requires_local_review";
const REVIEW_REASON_QUARANTINED: &str = "quarantined_memory_requires_decision";
const REVIEW_REASON_REMOTE_UPLOAD: &str = "remote_upload_requires_local_review";
const REVIEW_REASON_MISSING_SOURCE: &str = "missing_source_trace";
const REVIEW_REASON_UNSAFE_FLAG: &str = "unsafe_persistence_flag";
const REVIEW_ACTION_PROMOTE: &str = "promote";
const REVIEW_ACTION_QUARANTINE: &str = "quarantine";
const REVIEW_ACTION_FORGET: &str = "forget";
const REVIEW_ACTION_KEEP_REVIEWING: &str = "keep_reviewing";

/// `axon memory review` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryReviewCommand {
    /// 返回数量上限。
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryReviewReport {
    status: &'static str,
    limit: usize,
    pending_review_count: usize,
    records: Vec<MemoryReviewRecordView>,
    remote_review_allowed: bool,
    raw_payload_exported: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryReviewRecordView {
    record: MemoryRecordView,
    review_reasons: Vec<&'static str>,
    recommended_actions: Vec<&'static str>,
}

pub(super) fn run_memory_review(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryReviewCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let records = match store.pending_review_records_limited(limit) {
        Ok(records) => records,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let pending = records.iter().filter_map(review_record).collect::<Vec<_>>();
    let report = MemoryReviewReport {
        status: MEMORY_REVIEW_STATUS_READY,
        limit,
        pending_review_count: pending.len(),
        records: pending,
        remote_review_allowed: false,
        raw_payload_exported: false,
    };
    render_review(format, &report)
}

fn review_record(record: &MemorySqliteRecord) -> Option<MemoryReviewRecordView> {
    let reasons = review_reasons(record);
    if reasons.is_empty() {
        return None;
    }
    Some(MemoryReviewRecordView {
        recommended_actions: recommended_actions(record.record.state),
        record: record_view(record),
        review_reasons: reasons,
    })
}

fn review_reasons(record: &MemorySqliteRecord) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    match record.record.state {
        MemoryState::Weak => reasons.push(REVIEW_REASON_WEAK),
        MemoryState::Quarantined => reasons.push(REVIEW_REASON_QUARANTINED),
        MemoryState::Strong | MemoryState::Tombstone => {}
    }
    match record
        .record
        .source_trace
        .as_ref()
        .map(|trace| trace.origin)
    {
        Some(MemorySourceOrigin::RemoteUpload) => reasons.push(REVIEW_REASON_REMOTE_UPLOAD),
        Some(_) => {}
        None => reasons.push(REVIEW_REASON_MISSING_SOURCE),
    }
    if record.prompt_text_persisted || record.raw_payload_persisted || record.remote_write_allowed {
        reasons.push(REVIEW_REASON_UNSAFE_FLAG);
    }
    reasons
}

fn recommended_actions(state: MemoryState) -> Vec<&'static str> {
    match state {
        MemoryState::Weak => vec![REVIEW_ACTION_PROMOTE, REVIEW_ACTION_QUARANTINE],
        MemoryState::Quarantined => vec![REVIEW_ACTION_FORGET, REVIEW_ACTION_KEEP_REVIEWING],
        MemoryState::Strong => vec![REVIEW_ACTION_QUARANTINE],
        MemoryState::Tombstone => vec![REVIEW_ACTION_KEEP_REVIEWING],
    }
}

fn render_review(format: OutputFormat, report: &MemoryReviewReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            if report.records.is_empty() {
                stdout.push_str("没有待复核 Memory 记录\n");
            } else {
                for record in &report.records {
                    let _ = writeln!(
                        stdout,
                        "{}\t{}\t{}\t{}",
                        record.record.memory_id,
                        record.record.state,
                        record.review_reasons.join(","),
                        record.record.content_ref.as_deref().unwrap_or("-")
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
