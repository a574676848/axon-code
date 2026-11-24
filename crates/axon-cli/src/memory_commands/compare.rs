use std::fmt::Write as _;

use axon_agent_runtime::{MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED, MemorySqliteRecord};
use axon_domain::{MemoryId, MemoryState};
use clap::Args;
use serde::Serialize;

use crate::{
    CliRunResult, OutputFormat,
    management_commands::{cli_error, json_result},
};

use super::MemoryCliContext;

const MEMORY_COMPARE_STATUS_READY: &str = "memory_compare_ready";
const MEMORY_COMPARE_ACTION_KEEP_WINNER: &str = "keep_winner";
const MEMORY_COMPARE_ACTION_RECOVER_LEFT: &str = "recover_left";
const MEMORY_COMPARE_ACTION_RECOVER_RIGHT: &str = "recover_right";
const MEMORY_COMPARE_ACTION_REVIEW_MANUALLY: &str = "review_manually";
const MEMORY_COMPARE_SCOPE_ANY: &str = "memory-scope:any";
const MEMORY_COMPARE_REPO_ANY: &str = "repo-scope:any";
const MEMORY_COMPARE_USER_ANY: &str = "user-scope:any";

/// `axon memory compare` 参数。
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct MemoryCompareCommand {
    /// 左侧 memory id。
    pub left_memory_id: u64,
    /// 右侧 memory id。
    pub right_memory_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryCompareReport {
    status: &'static str,
    left: MemoryCompareRecordView,
    right: MemoryCompareRecordView,
    same_evidence_scope: bool,
    content_conflict: bool,
    winner_memory_id: Option<u64>,
    recommended_action: &'static str,
    storage_source: &'static str,
    ref_only: bool,
    remote_write_allowed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct MemoryCompareRecordView {
    memory_id: u64,
    content_ref: Option<String>,
    evidence_ref: Option<String>,
    memory_summary: Option<String>,
    namespace_ref: Option<String>,
    repo_ref: Option<String>,
    user_ref: Option<String>,
    status: &'static str,
    state: &'static str,
    recoverable_conflict: bool,
    updated_at_ms: u64,
    confidence_basis_points: u16,
    importance_basis_points: u16,
    rank_basis_points: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ComparableMemoryRecord {
    memory_id: u64,
    content_ref: Option<String>,
    evidence_ref: Option<String>,
    memory_summary: Option<String>,
    namespace_ref: Option<String>,
    repo_ref: Option<String>,
    user_ref: Option<String>,
    archived: bool,
    state: MemoryState,
    tombstone_reason: Option<String>,
    updated_at_ms: u64,
    confidence_basis_points: u16,
    importance_basis_points: u16,
    prompt_text_persisted: bool,
    raw_payload_persisted: bool,
    remote_write_allowed: bool,
}

pub(super) fn run_memory_compare(
    format: OutputFormat,
    context: &MemoryCliContext,
    command: &MemoryCompareCommand,
) -> CliRunResult {
    if command.left_memory_id == 0
        || command.right_memory_id == 0
        || command.left_memory_id == command.right_memory_id
    {
        return cli_error("memory compare 需要两个不同且大于 0 的 memory id");
    }
    let (left, right, storage_source) = match read_sqlite_pair(context, command) {
        Ok(pair) => pair,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let report = compare_report(&left, &right, storage_source);
    render_compare(format, &report)
}

fn read_sqlite_pair(
    context: &MemoryCliContext,
    command: &MemoryCompareCommand,
) -> axon_domain::AxonResult<(ComparableMemoryRecord, ComparableMemoryRecord, &'static str)> {
    let store = context.orchestrator.sqlite_store()?;
    let records = store.records_by_memory_ids(&[
        MemoryId(command.left_memory_id),
        MemoryId(command.right_memory_id),
    ])?;
    let left = records
        .iter()
        .find(|record| record.record.id.0 == command.left_memory_id)
        .ok_or_else(|| axon_domain::AxonError::runtime_failure("未找到左侧 Memory 记录"))?;
    let right = records
        .iter()
        .find(|record| record.record.id.0 == command.right_memory_id)
        .ok_or_else(|| axon_domain::AxonError::runtime_failure("未找到右侧 Memory 记录"))?;
    Ok((
        ComparableMemoryRecord::from(left),
        ComparableMemoryRecord::from(right),
        "sqlite",
    ))
}

fn compare_report(
    left: &ComparableMemoryRecord,
    right: &ComparableMemoryRecord,
    storage_source: &'static str,
) -> MemoryCompareReport {
    let same_evidence_scope = evidence_scope(left) == evidence_scope(right);
    let content_conflict = same_evidence_scope
        && left.content_ref.is_some()
        && right.content_ref.is_some()
        && left.content_ref != right.content_ref;
    let winner_memory_id = content_conflict.then(|| winner(left, right).memory_id);
    MemoryCompareReport {
        status: MEMORY_COMPARE_STATUS_READY,
        left: record_view(left),
        right: record_view(right),
        same_evidence_scope,
        content_conflict,
        winner_memory_id,
        recommended_action: recommended_action(left, right, content_conflict, winner_memory_id),
        storage_source,
        ref_only: ref_only(left) && ref_only(right),
        remote_write_allowed: left.remote_write_allowed || right.remote_write_allowed,
    }
}

fn render_compare(format: OutputFormat, report: &MemoryCompareReport) -> CliRunResult {
    match format {
        OutputFormat::Json => json_result(report),
        OutputFormat::Text => {
            let mut stdout = String::new();
            let _ = writeln!(
                stdout,
                "compare\t{} vs {}\tconflict={}\twinner={}",
                report.left.memory_id,
                report.right.memory_id,
                report.content_conflict,
                report
                    .winner_memory_id
                    .map_or_else(|| "-".to_owned(), |id| id.to_string())
            );
            let _ = writeln!(stdout, "action\t{}", report.recommended_action);
            CliRunResult {
                exit_code: 0,
                stdout,
                stderr: String::new(),
            }
        }
    }
}

fn record_view(record: &ComparableMemoryRecord) -> MemoryCompareRecordView {
    MemoryCompareRecordView {
        memory_id: record.memory_id,
        content_ref: record.content_ref.clone(),
        evidence_ref: record.evidence_ref.clone(),
        memory_summary: record.memory_summary.clone(),
        namespace_ref: record.namespace_ref.clone(),
        repo_ref: record.repo_ref.clone(),
        user_ref: record.user_ref.clone(),
        status: store_status_code(record.archived),
        state: memory_state_code(record.state),
        recoverable_conflict: recoverable_conflict(record),
        updated_at_ms: record.updated_at_ms,
        confidence_basis_points: record.confidence_basis_points,
        importance_basis_points: record.importance_basis_points,
        rank_basis_points: rank_basis_points(record),
    }
}

fn winner<'a>(
    left: &'a ComparableMemoryRecord,
    right: &'a ComparableMemoryRecord,
) -> &'a ComparableMemoryRecord {
    let left_rank = (
        state_rank(left.state),
        rank_basis_points(left),
        left.updated_at_ms,
        left.memory_id,
    );
    let right_rank = (
        state_rank(right.state),
        rank_basis_points(right),
        right.updated_at_ms,
        right.memory_id,
    );
    if right_rank > left_rank { right } else { left }
}

fn recommended_action(
    left: &ComparableMemoryRecord,
    right: &ComparableMemoryRecord,
    content_conflict: bool,
    winner_memory_id: Option<u64>,
) -> &'static str {
    if !content_conflict {
        return MEMORY_COMPARE_ACTION_REVIEW_MANUALLY;
    }
    let Some(winner_memory_id) = winner_memory_id else {
        return MEMORY_COMPARE_ACTION_REVIEW_MANUALLY;
    };
    if winner_memory_id == left.memory_id && recoverable_conflict(left) {
        return MEMORY_COMPARE_ACTION_RECOVER_LEFT;
    }
    if winner_memory_id == right.memory_id && recoverable_conflict(right) {
        return MEMORY_COMPARE_ACTION_RECOVER_RIGHT;
    }
    MEMORY_COMPARE_ACTION_KEEP_WINNER
}

fn evidence_scope(record: &ComparableMemoryRecord) -> (String, String, String, String) {
    (
        record
            .namespace_ref
            .clone()
            .unwrap_or_else(|| MEMORY_COMPARE_SCOPE_ANY.to_owned()),
        record
            .repo_ref
            .clone()
            .unwrap_or_else(|| MEMORY_COMPARE_REPO_ANY.to_owned()),
        record
            .user_ref
            .clone()
            .unwrap_or_else(|| MEMORY_COMPARE_USER_ANY.to_owned()),
        record
            .evidence_ref
            .clone()
            .or_else(|| record.memory_summary.clone())
            .unwrap_or_default(),
    )
}

const fn state_rank(state: MemoryState) -> u8 {
    match state {
        MemoryState::Strong => 2,
        MemoryState::Weak => 1,
        MemoryState::Quarantined | MemoryState::Tombstone => 0,
    }
}

fn rank_basis_points(record: &ComparableMemoryRecord) -> u32 {
    u32::from(record.confidence_basis_points) + u32::from(record.importance_basis_points)
}

fn recoverable_conflict(record: &ComparableMemoryRecord) -> bool {
    record.archived
        && record.tombstone_reason.as_deref() == Some(MEMORY_CONSOLIDATION_REASON_CONFLICT_ARCHIVED)
        && record
            .content_ref
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn ref_only(record: &ComparableMemoryRecord) -> bool {
    !record.prompt_text_persisted && !record.raw_payload_persisted
}

impl From<&MemorySqliteRecord> for ComparableMemoryRecord {
    fn from(record: &MemorySqliteRecord) -> Self {
        let source_trace = record.record.source_trace.as_ref();
        let tombstone = record.record.tombstone.as_ref();
        Self {
            memory_id: record.record.id.0,
            content_ref: record.record.content_ref.clone(),
            evidence_ref: source_trace.and_then(|trace| trace.evidence_ref.clone()),
            memory_summary: source_trace.map(|trace| trace.summary.clone()),
            namespace_ref: record.namespace_ref.clone(),
            repo_ref: record.repo_ref.clone(),
            user_ref: record.user_ref.clone(),
            archived: record.archived_at_ms.is_some(),
            state: record.record.state,
            tombstone_reason: tombstone.map(|value| value.reason.clone()),
            updated_at_ms: record.updated_at_ms,
            confidence_basis_points: record.confidence_basis_points,
            importance_basis_points: record.importance_basis_points,
            prompt_text_persisted: record.prompt_text_persisted,
            raw_payload_persisted: record.raw_payload_persisted,
            remote_write_allowed: record.remote_write_allowed,
        }
    }
}

const fn store_status_code(archived: bool) -> &'static str {
    if archived { "archived" } else { "active" }
}

const fn memory_state_code(state: MemoryState) -> &'static str {
    match state {
        MemoryState::Weak => "weak",
        MemoryState::Strong => "strong",
        MemoryState::Quarantined => "quarantined",
        MemoryState::Tombstone => "tombstone",
    }
}
