use std::fmt::Write as _;

use axon_agent_runtime::MemorySqliteRecord;
use axon_domain::{MemoryId, MemorySourceOrigin, MemoryState};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{
    MemoryCliContext,
    view::{MemoryRecordView, record_view},
};

const MEMORY_INSPECT_STATUS_READY: &str = "memory_inspect_ready";
const MEMORY_INSPECT_ACTION_APPROVE: &str = "approve";
const MEMORY_INSPECT_ACTION_APPROVE_PENDING: &str = "approve-pending";
const MEMORY_INSPECT_ACTION_ARCHIVE_PENDING: &str = "archive-pending";
const MEMORY_INSPECT_ACTION_QUARANTINE: &str = "quarantine";
const MEMORY_INSPECT_ACTION_FORGET: &str = "forget";
const MEMORY_INSPECT_ACTION_KEEP_REVIEWING: &str = "keep_reviewing";
const MEMORY_INSPECT_REASON_WEAK: &str = "weak_memory_requires_local_review";
const MEMORY_INSPECT_REASON_QUARANTINED: &str = "quarantined_memory_requires_decision";
const MEMORY_INSPECT_REASON_REMOTE_UPLOAD: &str = "remote_upload_requires_local_review";
const MEMORY_INSPECT_REASON_MISSING_SOURCE: &str = "missing_source_trace";
const MEMORY_INSPECT_REASON_UNSAFE_FLAG: &str = "unsafe_persistence_flag";

/// `axon memory inspect` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryInspectCommand {
    /// 目标 memory id。
    pub memory_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryInspectReport {
    status: &'static str,
    memory_id: u64,
    record: MemoryRecordView,
    evidence: MemoryEvidenceView,
    review_reasons: Vec<&'static str>,
    recommended_actions: Vec<&'static str>,
    raw_payload_exported: bool,
    memory_summary_available: bool,
    remote_review_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryEvidenceView {
    source_origin: Option<&'static str>,
    source_session_id: Option<u64>,
    source_cell_id: Option<u64>,
    memory_summary: Option<String>,
    evidence_ref: Option<String>,
    tombstone_blocked_ref: Option<String>,
    has_tombstone_reason: bool,
}

pub(super) fn run_memory_inspect(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryInspectCommand,
) -> CliRunResult {
    if command.memory_id == 0 {
        return cli_error("memory id 必须大于 0");
    }
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let Some(record) = (match store.get_record(MemoryId(command.memory_id)) {
        Ok(record) => record,
        Err(error) => return cli_error(format!("{error:?}")),
    }) else {
        return cli_error("未找到目标 Memory 记录");
    };
    let report = inspect_report(&record);
    render_inspect(format, &report)
}

fn inspect_report(record: &MemorySqliteRecord) -> MemoryInspectReport {
    MemoryInspectReport {
        status: MEMORY_INSPECT_STATUS_READY,
        memory_id: record.record.id.0,
        record: record_view(record),
        evidence: evidence_view(record),
        review_reasons: review_reasons(record),
        recommended_actions: recommended_actions(record),
        raw_payload_exported: false,
        memory_summary_available: record
            .record
            .source_trace
            .as_ref()
            .is_some_and(|trace| !trace.summary.trim().is_empty()),
        remote_review_allowed: false,
    }
}

fn evidence_view(record: &MemorySqliteRecord) -> MemoryEvidenceView {
    let source_trace = record.record.source_trace.as_ref();
    let tombstone = record.record.tombstone.as_ref();
    MemoryEvidenceView {
        source_origin: source_trace.map(|trace| source_origin_code(trace.origin)),
        source_session_id: source_trace.and_then(|trace| trace.session_id.map(|id| id.0)),
        source_cell_id: source_trace.and_then(|trace| trace.cell_id.map(|id| id.0)),
        memory_summary: source_trace
            .filter(|trace| !trace.summary.trim().is_empty())
            .map(|trace| trace.summary.clone()),
        evidence_ref: source_trace.and_then(|trace| trace.evidence_ref.clone()),
        tombstone_blocked_ref: tombstone.map(|value| value.blocked_ref.clone()),
        has_tombstone_reason: tombstone.is_some_and(|value| !value.reason.trim().is_empty()),
    }
}

fn review_reasons(record: &MemorySqliteRecord) -> Vec<&'static str> {
    let mut reasons = Vec::new();
    match record.record.state {
        MemoryState::Weak => reasons.push(MEMORY_INSPECT_REASON_WEAK),
        MemoryState::Quarantined => reasons.push(MEMORY_INSPECT_REASON_QUARANTINED),
        MemoryState::Strong | MemoryState::Tombstone => {}
    }
    match record
        .record
        .source_trace
        .as_ref()
        .map(|trace| trace.origin)
    {
        Some(MemorySourceOrigin::RemoteUpload) => reasons.push(MEMORY_INSPECT_REASON_REMOTE_UPLOAD),
        Some(_) => {}
        None => reasons.push(MEMORY_INSPECT_REASON_MISSING_SOURCE),
    }
    if record.prompt_text_persisted || record.raw_payload_persisted || record.remote_write_allowed {
        reasons.push(MEMORY_INSPECT_REASON_UNSAFE_FLAG);
    }
    reasons
}

fn recommended_actions(record: &MemorySqliteRecord) -> Vec<&'static str> {
    if record.prompt_text_persisted || record.raw_payload_persisted || record.remote_write_allowed {
        return vec![
            MEMORY_INSPECT_ACTION_QUARANTINE,
            MEMORY_INSPECT_ACTION_KEEP_REVIEWING,
        ];
    }
    match record.record.state {
        MemoryState::Weak if record.record.source_trace.is_some() => {
            vec![
                MEMORY_INSPECT_ACTION_APPROVE,
                MEMORY_INSPECT_ACTION_APPROVE_PENDING,
                MEMORY_INSPECT_ACTION_ARCHIVE_PENDING,
                MEMORY_INSPECT_ACTION_QUARANTINE,
            ]
        }
        MemoryState::Weak => vec![
            MEMORY_INSPECT_ACTION_ARCHIVE_PENDING,
            MEMORY_INSPECT_ACTION_QUARANTINE,
            MEMORY_INSPECT_ACTION_KEEP_REVIEWING,
        ],
        MemoryState::Strong => vec![MEMORY_INSPECT_ACTION_QUARANTINE],
        MemoryState::Quarantined => vec![
            MEMORY_INSPECT_ACTION_FORGET,
            MEMORY_INSPECT_ACTION_KEEP_REVIEWING,
        ],
        MemoryState::Tombstone => vec![MEMORY_INSPECT_ACTION_KEEP_REVIEWING],
    }
}

fn render_inspect(format: OutputFormat, report: &MemoryInspectReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "{}\t{}\t{}",
                report.memory_id,
                report.record.state,
                report.record.content_ref.as_deref().unwrap_or("-")
            );
            let _ = writeln!(
                stdout,
                "evidence\t{}\t{}",
                report.evidence.evidence_ref.as_deref().unwrap_or("-"),
                report.evidence.memory_summary.as_deref().unwrap_or("-")
            );
            let _ = writeln!(stdout, "actions\t{}", report.recommended_actions.join(","));
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

const fn source_origin_code(origin: MemorySourceOrigin) -> &'static str {
    match origin {
        MemorySourceOrigin::LocalTimeline => "local_timeline",
        MemorySourceOrigin::AgentUserContext => "agent_user_context",
        MemorySourceOrigin::AgentExperience => "agent_experience",
        MemorySourceOrigin::AgentKnowledge => "agent_knowledge",
        MemorySourceOrigin::AgentProcedure => "agent_procedure",
        MemorySourceOrigin::ExplicitUserPreference => "explicit_user_preference",
        MemorySourceOrigin::DebuggingExperience => "debugging_experience",
        MemorySourceOrigin::RemoteUpload => "remote_upload",
        MemorySourceOrigin::CommandCompletionFeedback => "command_completion_feedback",
        MemorySourceOrigin::SkillReflection => "skill_reflection",
    }
}
