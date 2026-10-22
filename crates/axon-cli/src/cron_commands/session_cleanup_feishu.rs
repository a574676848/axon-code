use axon_remote_feishu::{
    RemoteFeishuAgentReplyArtifactRuntime, RemoteFeishuFeedbackTaskStoreRuntime,
};
use axon_remote_runtime::RemoteNotificationOutboxRuntime;
use std::path::Path;

use super::SessionCleanupReport;

#[derive(Debug, Default)]
struct FeishuSessionCleanupReport {
    reply_artifacts: usize,
    outbox_records: usize,
    feedback_records: usize,
}

pub(super) fn apply_feishu_cleanup(
    root: &Path,
    stale_sessions: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let result = cleanup_feishu_session_data(root, stale_sessions)?;
    report.deleted_feishu_reply_artifact_count = report
        .deleted_feishu_reply_artifact_count
        .saturating_add(result.reply_artifacts);
    report.deleted_feishu_outbox_count = report
        .deleted_feishu_outbox_count
        .saturating_add(result.outbox_records);
    report.deleted_feishu_feedback_count = report
        .deleted_feishu_feedback_count
        .saturating_add(result.feedback_records);
    Ok(())
}

pub(super) fn apply_feishu_cleanup_count(
    root: &Path,
    stale_sessions: &[String],
    report: &mut SessionCleanupReport,
) -> Result<(), String> {
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root);
    let artifact_path = RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&runtime_root);
    let dispatch_ids = RemoteFeishuAgentReplyArtifactRuntime::dispatches_for_run_sessions(
        &artifact_path,
        stale_sessions,
    )
    .map_err(|error| format!("统计 Feishu 回复 artifact 失败：{error:?}"))?;
    report.deleted_feishu_reply_artifact_count = dispatch_ids.len();
    report.deleted_feishu_outbox_count = axon_remote_runtime::RemoteStateStoreRuntime::count_keys(
        RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root),
        &dispatch_ids,
    )
    .map_err(|error| format!("统计 Feishu outbox 失败：{error:?}"))?;
    report.deleted_feishu_feedback_count =
        axon_remote_runtime::RemoteStateStoreRuntime::count_keys(
            RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&runtime_root),
            &dispatch_ids,
        )
        .map_err(|error| format!("统计 Feishu feedback 失败：{error:?}"))?;
    Ok(())
}

fn cleanup_feishu_session_data(
    root: &Path,
    session_ids: &[String],
) -> Result<FeishuSessionCleanupReport, String> {
    if session_ids.is_empty() {
        return Ok(FeishuSessionCleanupReport::default());
    }
    let runtime_root = axon_remote_feishu::remote_feishu_runtime_root(root);
    let artifact_path = RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(&runtime_root);
    let dispatch_ids = RemoteFeishuAgentReplyArtifactRuntime::dispatches_for_run_sessions(
        &artifact_path,
        session_ids,
    )
    .map_err(|error| format!("清理 Feishu 回复 artifact 失败：{error:?}"))?;
    let outbox_records = RemoteNotificationOutboxRuntime::remove_for_dispatches(
        RemoteNotificationOutboxRuntime::resolve_store_path(&runtime_root),
        &dispatch_ids,
    )
    .map_err(|error| format!("清理 Feishu outbox 失败：{error:?}"))?;
    let feedback_records = RemoteFeishuFeedbackTaskStoreRuntime::remove_for_dispatches(
        RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&runtime_root),
        &dispatch_ids,
    )
    .map_err(|error| format!("清理 Feishu feedback 失败：{error:?}"))?;
    let reply_artifacts =
        RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(artifact_path, &dispatch_ids)
            .map_err(|error| format!("删除 Feishu 回复 artifact 失败：{error:?}"))?;
    Ok(FeishuSessionCleanupReport {
        reply_artifacts,
        outbox_records,
        feedback_records,
    })
}
