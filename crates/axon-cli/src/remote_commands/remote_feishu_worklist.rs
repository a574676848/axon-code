use axon_remote_feishu::{
    RemoteFeishuGovernanceDecisionStatus, RemoteFeishuGovernanceRecord,
    RemoteFeishuGovernanceStoreRuntime, RemoteFeishuThreadState,
};
use axon_remote_runtime::{RemoteNotificationOutboxRuntime, RemoteTransportKind};

use crate::{CliRunResult, OutputFormat, management_commands::cli_error};

use super::{
    RemoteCliContext, bounded_limit,
    remote_feishu::{RemoteFeishuWorklistItem, RemoteFeishuWorklistReport},
};

pub(super) fn run_remote_feishu_worklist(
    format: OutputFormat,
    context: &RemoteCliContext,
    command: &super::RemoteListCommand,
) -> CliRunResult {
    let limit = match bounded_limit(command.limit) {
        Ok(limit) => limit,
        Err(error) => return cli_error(error),
    };
    let mut records =
        match RemoteNotificationOutboxRuntime::latest_records(&context.outbox_store_path) {
            Ok(records) => records
                .into_iter()
                .filter(|record| record.transport == RemoteTransportKind::Feishu)
                .collect::<Vec<_>>(),
            Err(error) => return cli_error(format!("{error:?}")),
        };
    records.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    let pending_count = records
        .iter()
        .filter(|record| {
            record.status == axon_remote_runtime::RemoteNotificationDispatchStatus::Pending
        })
        .count();
    let retryable_count = records
        .iter()
        .filter(|record| {
            record.status == axon_remote_runtime::RemoteNotificationDispatchStatus::Failed
        })
        .count();
    let governance_records = match RemoteFeishuGovernanceStoreRuntime::read_records(
        RemoteFeishuGovernanceStoreRuntime::resolve_store_path(&context.root),
    ) {
        Ok(records) => records,
        Err(error) => return cli_error(format!("{error:?}")),
    };
    let latest_threads = RemoteFeishuGovernanceStoreRuntime::latest_threads(&governance_records);
    let processing_threads = latest_threads
        .iter()
        .filter(|record| record.thread_state == RemoteFeishuThreadState::Processing)
        .count();
    let queued_threads = latest_threads
        .iter()
        .filter(|record| record.thread_state == RemoteFeishuThreadState::Queued)
        .count();
    let rejected_threads = latest_threads
        .iter()
        .filter(|record| record.thread_state == RemoteFeishuThreadState::Rejected)
        .count();
    let report = RemoteFeishuWorklistReport {
        status: "remote_feishu_worklist_ready",
        total_records: records.len(),
        pending_count,
        retryable_count,
        governance_threads: latest_threads.len(),
        processing_threads,
        queued_threads,
        rejected_threads,
        recent_dispatches: records
            .iter()
            .take(limit)
            .map(|record| RemoteFeishuWorklistItem {
                chat_id_sha256: governance_chat_id_for_session(&latest_threads, record.session_id),
                dispatch_id: record.dispatch_id.clone(),
                session_id: record.session_id,
                status: match record.status {
                    axon_remote_runtime::RemoteNotificationDispatchStatus::Pending => "pending",
                    axon_remote_runtime::RemoteNotificationDispatchStatus::Attempting => {
                        "attempting"
                    }
                    axon_remote_runtime::RemoteNotificationDispatchStatus::Delivered => "delivered",
                    axon_remote_runtime::RemoteNotificationDispatchStatus::Failed => "failed",
                    axon_remote_runtime::RemoteNotificationDispatchStatus::Abandoned => "abandoned",
                },
                governance_status: governance_status_for_session(
                    &latest_threads,
                    record.session_id,
                ),
                governance_reason: governance_reason_for_session(
                    &latest_threads,
                    record.session_id,
                ),
                text_preview: governance_preview_for_session(&latest_threads, record.session_id),
                mentioned: governance_mention_for_session(&latest_threads, record.session_id),
                retryable: record.status
                    == axon_remote_runtime::RemoteNotificationDispatchStatus::Failed,
                next_retry_after_ms: record.next_retry_after_ms,
                has_adapter_message_id: record.adapter_message_id.is_some(),
            })
            .collect(),
    };
    super::remote_render::render_remote_feishu_worklist(format, &report)
}

fn governance_status_for_session(
    records: &[RemoteFeishuGovernanceRecord],
    session_id: u64,
) -> Option<&'static str> {
    records
        .iter()
        .find(|record| record.session_id == session_id)
        .map(|record| governance_status_code(record.decision_status))
}

fn governance_reason_for_session(
    records: &[RemoteFeishuGovernanceRecord],
    session_id: u64,
) -> Option<String> {
    records
        .iter()
        .find(|record| record.session_id == session_id)
        .map(|record| record.reason.clone())
}

fn governance_preview_for_session(
    records: &[RemoteFeishuGovernanceRecord],
    session_id: u64,
) -> Option<String> {
    records
        .iter()
        .find(|record| record.session_id == session_id)
        .map(|record| record.text_preview.clone())
}

fn governance_mention_for_session(
    records: &[RemoteFeishuGovernanceRecord],
    session_id: u64,
) -> Option<bool> {
    records
        .iter()
        .find(|record| record.session_id == session_id)
        .map(|record| record.mentioned)
}

fn governance_chat_id_for_session(
    records: &[RemoteFeishuGovernanceRecord],
    session_id: u64,
) -> String {
    records
        .iter()
        .find(|record| record.session_id == session_id)
        .map(|record| record.chat_id_sha256.clone())
        .unwrap_or_default()
}

const fn governance_status_code(status: RemoteFeishuGovernanceDecisionStatus) -> &'static str {
    match status {
        RemoteFeishuGovernanceDecisionStatus::Accepted => "accepted",
        RemoteFeishuGovernanceDecisionStatus::DuplicateDropped => "duplicateDropped",
        RemoteFeishuGovernanceDecisionStatus::RejectedByAllowlist => "rejectedByAllowlist",
        RemoteFeishuGovernanceDecisionStatus::RejectedByBlacklist => "rejectedByBlacklist",
        RemoteFeishuGovernanceDecisionStatus::RejectedByMentionGate => "rejectedByMentionGate",
        RemoteFeishuGovernanceDecisionStatus::IgnoredSelfEvent => "ignoredSelfEvent",
    }
}
