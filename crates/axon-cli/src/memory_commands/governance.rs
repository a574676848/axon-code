use axon_agent_runtime::{MemorySqliteRecord, MemorySqliteRecordFilter};
use axon_domain::{MemoryId, MemoryState, MemoryTombstone};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::{
    MemoryCliContext, bounded_limit, now_ms,
    view::{MemoryRecordView, memory_state_code, record_view},
};

const MEMORY_STATUS_UPDATED: &str = "memory_updated";
const MEMORY_STATUS_BATCH_PREVIEW: &str = "memory_batch_preview";
const MEMORY_STATUS_BATCH_UPDATED: &str = "memory_batch_updated";
const MEMORY_DEFAULT_APPROVE_REASON: &str = "local_cli_approve";
const MEMORY_DEFAULT_ARCHIVE_REASON: &str = "local_cli_archive_pending";
const MEMORY_DEFAULT_QUARANTINE_REASON: &str = "local_cli_quarantine";
const MEMORY_DEFAULT_FORGET_REASON: &str = "local_cli_forget";
const MEMORY_APPROVE_PENDING_REQUIRES_YES: &str = "approve-pending 需要 --yes 才会写入";
const MEMORY_ARCHIVE_PENDING_REQUIRES_YES: &str = "archive-pending 需要 --yes 才会写入";

/// `axon memory approve|quarantine|forget` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryGovernanceCommand {
    /// 目标 memory id。
    pub memory_id: u64,
    /// 治理原因；不得包含原文或 secret。
    #[arg(long)]
    pub reason: Option<String>,
}

/// `axon memory approve-pending` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryApprovePendingCommand {
    /// 本轮最多批准的 weak 记录数量。
    #[arg(long)]
    pub limit: Option<usize>,
    /// 显式确认批量写入；未提供时只预览。
    #[arg(long)]
    pub yes: bool,
    /// 治理原因；不得包含原文或 secret。
    #[arg(long)]
    pub reason: Option<String>,
}

/// `axon memory archive-pending` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryArchivePendingCommand {
    /// 本轮最多归档的待复核记录数量。
    #[arg(long)]
    pub limit: Option<usize>,
    /// 显式确认批量写入；未提供时只预览。
    #[arg(long)]
    pub yes: bool,
    /// 治理原因；不得包含原文或 secret。
    #[arg(long)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryGovernanceReport {
    status: &'static str,
    action: &'static str,
    memory_id: u64,
    previous_state: &'static str,
    record: MemoryRecordView,
    content_ref_cleared: bool,
    vector_index_cleared: bool,
    tombstone_written: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryApprovePendingReport {
    status: &'static str,
    action: &'static str,
    limit: usize,
    approved_count: usize,
    skipped_count: usize,
    requires_yes: bool,
    records: Vec<MemoryRecordView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryArchivePendingReport {
    status: &'static str,
    action: &'static str,
    limit: usize,
    archived_count: usize,
    skipped_count: usize,
    requires_yes: bool,
    records: Vec<MemoryRecordView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MemoryGovernanceAction {
    Approve,
    Quarantine,
    Forget,
}

pub(super) fn run_memory_governance(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryGovernanceCommand,
    action: MemoryGovernanceAction,
) -> CliRunResult {
    if command.memory_id == 0 {
        return cli_error("memory id 必须大于 0");
    }
    let reason = match governance_reason(command.reason.as_deref(), action) {
        Ok(reason) => reason,
        Err(error) => return cli_error(error),
    };
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let Some(mut record) = (match store.get_record(MemoryId(command.memory_id)) {
        Ok(record) => record,
        Err(error) => return cli_error(format!("{error:?}")),
    }) else {
        return cli_error("未找到目标 Memory 记录");
    };

    let previous_state = memory_state_code(record.record.state);
    let applied_at_ms = now_ms().max(record.created_at_ms);
    if let Err(error) = apply_governance_action(&mut record, action, &reason, applied_at_ms) {
        return cli_error(error);
    }
    if let Err(error) = store.update_governance_records(std::slice::from_ref(&record)) {
        return cli_error(format!("{error:?}"));
    }

    let report = MemoryGovernanceReport {
        status: MEMORY_STATUS_UPDATED,
        action: governance_action_code(action),
        memory_id: command.memory_id,
        previous_state,
        record: record_view(&record),
        content_ref_cleared: governance_clears_recall(action)
            && record.record.content_ref.is_none(),
        vector_index_cleared: governance_clears_recall(action) && record.vector_id.is_none(),
        tombstone_written: record.record.tombstone.is_some(),
    };
    render_governance(format, &report)
}

pub(super) fn run_memory_approve_pending(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryApprovePendingCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let reason = match governance_reason(command.reason.as_deref(), MemoryGovernanceAction::Approve)
    {
        Ok(reason) => reason,
        Err(error) => return cli_error(error),
    };
    let store = match context.orchestrator.sqlite_store() {
        Ok(store) => store,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let records = match store.records_limited(MemorySqliteRecordFilter::Weak, limit) {
        Ok(records) => records,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let mut skipped_count = 0usize;
    let mut approved_records = Vec::new();
    let applied_at_ms = now_ms();
    for record in records {
        let mut candidate = record;
        let applied_at_ms = applied_at_ms.max(candidate.created_at_ms);
        if apply_governance_action(
            &mut candidate,
            MemoryGovernanceAction::Approve,
            &reason,
            applied_at_ms,
        )
        .is_ok()
        {
            approved_records.push(candidate);
        } else {
            skipped_count = skipped_count.saturating_add(1);
        }
    }
    if !command.yes {
        return render_approve_pending(
            format,
            &approve_pending_report(
                MEMORY_STATUS_BATCH_PREVIEW,
                limit,
                &approved_records,
                skipped_count,
                true,
            ),
        );
    }
    if let Err(error) = store.update_governance_records(&approved_records) {
        return cli_error(format!("{error:?}"));
    }
    render_approve_pending(
        format,
        &approve_pending_report(
            MEMORY_STATUS_BATCH_UPDATED,
            limit,
            &approved_records,
            skipped_count,
            false,
        ),
    )
}

pub(super) fn run_memory_archive_pending(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryArchivePendingCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let reason = match governance_reason_with_default(
        command.reason.as_deref(),
        MEMORY_DEFAULT_ARCHIVE_REASON,
    ) {
        Ok(reason) => reason,
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
    let mut skipped_count = 0usize;
    let mut archived_records = Vec::new();
    let applied_at_ms = now_ms();
    for record in records {
        let mut candidate = record;
        let applied_at_ms = applied_at_ms.max(candidate.created_at_ms);
        if apply_governance_action(
            &mut candidate,
            MemoryGovernanceAction::Quarantine,
            &reason,
            applied_at_ms,
        )
        .is_ok()
        {
            archived_records.push(candidate);
        } else {
            skipped_count = skipped_count.saturating_add(1);
        }
    }
    if !command.yes {
        return render_archive_pending(
            format,
            &archive_pending_report(
                MEMORY_STATUS_BATCH_PREVIEW,
                limit,
                &archived_records,
                skipped_count,
                true,
            ),
        );
    }
    if let Err(error) = store.update_governance_records(&archived_records) {
        return cli_error(format!("{error:?}"));
    }
    render_archive_pending(
        format,
        &archive_pending_report(
            MEMORY_STATUS_BATCH_UPDATED,
            limit,
            &archived_records,
            skipped_count,
            false,
        ),
    )
}

fn render_governance(format: OutputFormat, report: &MemoryGovernanceReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => CliRunResult {
            exit_code: 0,
            stdout: format!(
                "Memory {} applied: id={} {} -> {}\n",
                report.action, report.memory_id, report.previous_state, report.record.state
            ),
            stderr: String::new(),
        },
    }
}

fn render_approve_pending(
    format: OutputFormat,
    report: &MemoryApprovePendingReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let prefix = if report.requires_yes {
                MEMORY_APPROVE_PENDING_REQUIRES_YES
            } else {
                "approve-pending 已完成"
            };
            CliRunResult {
                exit_code: 0,
                stdout: format!(
                    "{prefix}: approved={} skipped={} limit={}\n",
                    report.approved_count, report.skipped_count, report.limit
                ),
                stderr: String::new(),
            }
        }
    }
}

fn render_archive_pending(
    format: OutputFormat,
    report: &MemoryArchivePendingReport,
) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let prefix = if report.requires_yes {
                MEMORY_ARCHIVE_PENDING_REQUIRES_YES
            } else {
                "archive-pending 已完成"
            };
            CliRunResult {
                exit_code: 0,
                stdout: format!(
                    "{prefix}: archived={} skipped={} limit={}\n",
                    report.archived_count, report.skipped_count, report.limit
                ),
                stderr: String::new(),
            }
        }
    }
}

fn approve_pending_report(
    status: &'static str,
    limit: usize,
    records: &[MemorySqliteRecord],
    skipped_count: usize,
    requires_yes: bool,
) -> MemoryApprovePendingReport {
    MemoryApprovePendingReport {
        status,
        action: "approve-pending",
        limit,
        approved_count: records.len(),
        skipped_count,
        requires_yes,
        records: records.iter().map(record_view).collect(),
    }
}

fn archive_pending_report(
    status: &'static str,
    limit: usize,
    records: &[MemorySqliteRecord],
    skipped_count: usize,
    requires_yes: bool,
) -> MemoryArchivePendingReport {
    MemoryArchivePendingReport {
        status,
        action: "archive-pending",
        limit,
        archived_count: records.len(),
        skipped_count,
        requires_yes,
        records: records.iter().map(record_view).collect(),
    }
}

fn apply_governance_action(
    record: &mut MemorySqliteRecord,
    action: MemoryGovernanceAction,
    reason: &str,
    now_ms: u64,
) -> Result<(), &'static str> {
    if record.record.state == MemoryState::Tombstone {
        return Err("tombstone Memory 不能重复治理");
    }
    match action {
        MemoryGovernanceAction::Approve => {
            if record.record.state != MemoryState::Weak {
                return Err("只有 weak Memory 可以本机批准为 strong");
            }
            if record
                .record
                .content_ref
                .as_ref()
                .is_none_or(|content_ref| content_ref.trim().is_empty())
            {
                return Err("批准 Memory 必须保留 content ref");
            }
            if record.record.source_trace.is_none() {
                return Err("批准 Memory 必须有 source trace");
            }
            if record.record.tombstone.is_some() {
                return Err("批准 Memory 不能带 tombstone");
            }
            if record.prompt_text_persisted
                || record.raw_payload_persisted
                || record.remote_write_allowed
            {
                return Err("批准 Memory 不能持久化原文、原始 payload 或允许远控写入");
            }
            record.record.state = MemoryState::Strong;
        }
        MemoryGovernanceAction::Quarantine => {
            let blocked_ref = governance_blocked_ref(record)?;
            record.record.state = MemoryState::Quarantined;
            record.record.tombstone = Some(MemoryTombstone {
                blocked_ref,
                reason: reason.to_owned(),
            });
        }
        MemoryGovernanceAction::Forget => {
            let blocked_ref = governance_blocked_ref(record)?;
            record.record.state = MemoryState::Tombstone;
            record.record.tombstone = Some(MemoryTombstone {
                blocked_ref,
                reason: reason.to_owned(),
            });
        }
    }
    if governance_clears_recall(action) {
        record.record.content_ref = None;
        record.vector_id = None;
    }
    record.updated_at_ms = now_ms;
    record.last_active_at_ms = now_ms;
    Ok(())
}

fn governance_blocked_ref(record: &MemorySqliteRecord) -> Result<String, &'static str> {
    record
        .record
        .content_ref
        .clone()
        .or_else(|| {
            record
                .record
                .tombstone
                .as_ref()
                .map(|tombstone| tombstone.blocked_ref.clone())
        })
        .ok_or("目标 Memory 缺少可治理引用")
}

fn governance_reason(
    reason: Option<&str>,
    action: MemoryGovernanceAction,
) -> Result<String, &'static str> {
    let default_reason = match action {
        MemoryGovernanceAction::Approve => MEMORY_DEFAULT_APPROVE_REASON,
        MemoryGovernanceAction::Quarantine => MEMORY_DEFAULT_QUARANTINE_REASON,
        MemoryGovernanceAction::Forget => MEMORY_DEFAULT_FORGET_REASON,
    };
    governance_reason_with_default(reason, default_reason)
}

fn governance_reason_with_default(
    reason: Option<&str>,
    default_reason: &'static str,
) -> Result<String, &'static str> {
    Ok(reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_reason)
        .to_owned())
}

const fn governance_action_code(action: MemoryGovernanceAction) -> &'static str {
    match action {
        MemoryGovernanceAction::Approve => "approve",
        MemoryGovernanceAction::Quarantine => "quarantine",
        MemoryGovernanceAction::Forget => "forget",
    }
}

const fn governance_clears_recall(action: MemoryGovernanceAction) -> bool {
    matches!(
        action,
        MemoryGovernanceAction::Quarantine | MemoryGovernanceAction::Forget
    )
}
