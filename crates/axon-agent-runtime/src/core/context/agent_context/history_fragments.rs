//! Agent context 的历史与 repo structure 片段。

use crate::{AgentPaneApplyHistoryRecord, AgentPaneChatHistoryRecord, RecentCommandState};

pub(super) fn apply_history_summary(history: &[AgentPaneApplyHistoryRecord]) -> Option<String> {
    let lines = history
        .iter()
        .map(|record| {
            format!(
                "apply path={} execution_request={} status={} audit_present={} source_ref={}",
                record.path,
                record.execution_request_id,
                record.status,
                record.materialize_audit_present,
                record.source_ref
            )
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(super) fn chat_history_summary(history: &[AgentPaneChatHistoryRecord]) -> Option<String> {
    let lines = history
        .iter()
        .map(|record| {
            format!(
                "chat cell={} prompt=\"{}\" response=\"{}\" source_ref={} response_truncated={}",
                record.cell_id,
                record.prompt_preview,
                record.response_preview,
                record.source_ref,
                record.response_truncated
            )
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(super) fn history_summary(history: &[RecentCommandState]) -> Option<String> {
    let lines = history
        .iter()
        .map(|command| {
            let exit_code = command
                .exit_code
                .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
            format!("cmd={} exit={exit_code}", command.command)
        })
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(super) fn repo_structure_source_ref(source_refs: &[String]) -> String {
    if source_refs.is_empty() {
        return "repo://unknown".to_owned();
    }
    source_refs.join(",")
}
