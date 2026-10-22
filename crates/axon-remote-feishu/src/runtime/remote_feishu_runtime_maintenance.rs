//! Feishu 远控运行态维护。
//!
//! SQLite 状态账本由 WAL 自动 checkpoint；维护器清理终态状态行和运行态日志。

use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axon_remote_runtime::{RemoteNotificationDispatchStatus, RemoteNotificationOutboxRuntime};

/// Feishu 常驻 gateway 运行态维护周期。
pub const REMOTE_FEISHU_MAINTENANCE_INTERVAL_MS: u64 = 900_000;
/// Feishu 终态状态和日志保留天数。
pub const REMOTE_FEISHU_LOG_RETENTION_DAYS: u64 = 7;
/// Feishu 日志目录最多保留的日志文件数。
pub const REMOTE_FEISHU_LOG_MAX_FILES: usize = 20;

const LOG_DIR: &str = "logs";
const SECONDS_PER_DAY: u64 = 86_400;

/// Feishu 运行态维护报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuRuntimeMaintenanceReport {
    /// Feishu 运行态目录。
    pub runtime_dir: PathBuf,
    /// 本次删除的过期文件数。
    pub removed_files: usize,
    /// 本次删除的终态状态行数。
    pub removed_state_records: usize,
}

/// Feishu 运行态维护 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuRuntimeMaintenanceRuntime;

impl RemoteFeishuRuntimeMaintenanceRuntime {
    /// 执行一次 Feishu 远控运行态维护。
    pub fn run_once(
        root: impl AsRef<Path>,
    ) -> Result<RemoteFeishuRuntimeMaintenanceReport, String> {
        let runtime_dir = crate::remote_feishu_runtime_root(root);
        fs::create_dir_all(&runtime_dir)
            .map_err(|error| format!("创建 Feishu 运行态目录失败: {error}"))?;
        let removed_state_records = remove_expired_state_records(&runtime_dir)?;
        Ok(RemoteFeishuRuntimeMaintenanceReport {
            removed_files: remove_expired_runtime_files(&runtime_dir)?,
            removed_state_records,
            runtime_dir,
        })
    }
}

fn remove_expired_runtime_files(runtime_dir: &Path) -> Result<usize, String> {
    remove_old_log_files(&runtime_dir.join(LOG_DIR))
}

fn remove_expired_state_records(runtime_dir: &Path) -> Result<usize, String> {
    let cutoff_ms = state_retention_cutoff_ms()?;
    let removed = remove_expired_outbox_artifacts(runtime_dir, cutoff_ms)?;
    let removed = removed
        + RemoteNotificationOutboxRuntime::remove_terminal_records_before(
            RemoteNotificationOutboxRuntime::resolve_store_path(runtime_dir),
            cutoff_ms,
        )
        .map_err(|error| format!("清理 Feishu outbox 终态记录失败: {error:?}"))?;
    let removed = removed
        + crate::RemoteFeishuFeedbackTaskStoreRuntime::remove_terminal_records_before(
            crate::RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(runtime_dir),
            cutoff_ms,
        )
        .map_err(|error| format!("清理 Feishu feedback 终态记录失败: {error:?}"))?;
    let removed = removed
        + crate::RemoteFeishuSchedulerStoreRuntime::remove_inactive_records_before(
            crate::RemoteFeishuSchedulerStoreRuntime::resolve_store_path(runtime_dir),
            cutoff_ms,
        )
        .map_err(|error| format!("清理 Feishu chat lease 终态记录失败: {error:?}"))?;
    let removed = removed
        + crate::RemoteFeishuProcessingStateStoreRuntime::remove_inactive_records_before(
            crate::RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(runtime_dir),
            cutoff_ms,
        )
        .map_err(|error| format!("清理 Feishu processing 终态记录失败: {error:?}"))?;
    Ok(removed)
}

fn remove_expired_outbox_artifacts(runtime_dir: &Path, cutoff_ms: u64) -> Result<usize, String> {
    let outbox_path = RemoteNotificationOutboxRuntime::resolve_store_path(runtime_dir);
    let dispatch_ids = RemoteNotificationOutboxRuntime::read_records(outbox_path)
        .map_err(|error| format!("读取 Feishu outbox 终态记录失败: {error:?}"))?
        .into_iter()
        .filter(|record| {
            matches!(
                record.status,
                RemoteNotificationDispatchStatus::Delivered
                    | RemoteNotificationDispatchStatus::Abandoned
            ) && record.updated_at_ms < cutoff_ms
        })
        .map(|record| record.dispatch_id)
        .collect::<Vec<_>>();
    crate::RemoteFeishuAgentReplyArtifactRuntime::remove_for_dispatches(
        crate::RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(runtime_dir),
        &dispatch_ids,
    )
    .map_err(|error| format!("清理 Feishu 过期回复 artifact 失败: {error:?}"))
}

fn state_retention_cutoff_ms() -> Result<u64, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("读取 Feishu 状态保留时间失败: {error}"))?;
    let retained = Duration::from_secs(REMOTE_FEISHU_LOG_RETENTION_DAYS * SECONDS_PER_DAY);
    Ok(u64::try_from(now.saturating_sub(retained).as_millis()).unwrap_or_default())
}

fn remove_old_log_files(log_dir: &Path) -> Result<usize, String> {
    if !log_dir.is_dir() {
        return Ok(0);
    }
    let mut logs = runtime_log_files(log_dir)?;
    let mut removed = 0;
    for path in logs
        .iter()
        .filter(|path| is_older_than_days(path, REMOTE_FEISHU_LOG_RETENTION_DAYS))
    {
        fs::remove_file(path).map_err(|error| format!("删除 Feishu 日志失败: {error}"))?;
        removed += 1;
    }
    logs = runtime_log_files(log_dir)?;
    logs.sort_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    while logs.len() > REMOTE_FEISHU_LOG_MAX_FILES {
        fs::remove_file(logs.remove(0))
            .map_err(|error| format!("删除 Feishu 过量日志失败: {error}"))?;
        removed += 1;
    }
    Ok(removed)
}

fn runtime_log_files(log_dir: &Path) -> Result<Vec<PathBuf>, String> {
    Ok(fs::read_dir(log_dir)
        .map_err(|error| format!("读取 Feishu 日志目录失败: {error}"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && is_log_file(path))
        .collect())
}

fn is_log_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "log")
}

fn is_older_than_days(path: &Path, days: u64) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > Duration::from_secs(days * SECONDS_PER_DAY))
}

#[cfg(test)]
mod tests {
    use axon_domain::SessionId;
    use axon_remote_runtime::{
        RemoteNotificationDelivery, RemoteNotificationDispatchStatus, RemoteNotificationFanoutPlan,
        RemoteNotificationKind, RemoteNotificationOutboxInput, RemoteNotificationOutboxRecord,
        RemoteNotificationSeverity, RemoteTransportKind, WireLifecycleStatus,
        remote_notification_wire_lifecycle_event,
    };

    use super::*;

    #[test]
    fn maintenance_removes_expired_terminal_outbox_and_reply_artifact_together() {
        let directory = axon_test_support::test_temp_dir();
        let dispatch_id = "expired-dispatch";
        let outbox = RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
            dispatch_id: dispatch_id.to_owned(),
            plan: RemoteNotificationFanoutPlan {
                session_id: SessionId(1),
                subscriber_id: "feishu-gateway".to_owned(),
                connection_id: "oc_chat".to_owned(),
                pairing_id: "pairing".to_owned(),
                transport: RemoteTransportKind::Feishu,
                subscribed_kinds: vec![RemoteNotificationKind::ThreadCompleted],
                delivered: vec![RemoteNotificationDelivery {
                    kind: RemoteNotificationKind::ThreadCompleted,
                    wire_event: remote_notification_wire_lifecycle_event(
                        SessionId(1),
                        dispatch_id,
                        WireLifecycleStatus::Completed,
                        "完成",
                        Some("摘要".to_owned()),
                        1,
                    ),
                    bounded_body: "摘要".to_owned(),
                    reference_ids: vec!["artifact".to_owned()],
                    local_action_hint: "投递 Feishu 回复".to_owned(),
                    severity: RemoteNotificationSeverity::Info,
                }],
                dropped_count: 0,
                local_only: true,
                network_send_allowed: false,
                raw_payload_forward_allowed: false,
            },
            status: RemoteNotificationDispatchStatus::Delivered,
            recorded_at_ms: 1,
            updated_at_ms: 1,
            attempt_count: 1,
            next_retry_after_ms: None,
            last_error: None,
            adapter_message_id: None,
            source_trace: "maintenance-test".to_owned(),
            reason: "maintenance-test".to_owned(),
        })
        .expect("outbox 记录");
        let artifact = crate::RemoteFeishuAgentReplyArtifactRuntime::record_from_input(
            crate::RemoteFeishuAgentReplyArtifactInput {
                dispatch_id: dispatch_id.to_owned(),
                run_session_id: "remote-session".to_owned(),
                processing_message_id: None,
                target: crate::RemoteFeishuMessageTarget {
                    receive_id: "oc_chat".to_owned(),
                    receive_id_type: "chat_id".to_owned(),
                    reply_to_message_id: None,
                },
                thread_id: None,
                body: "完整回复".to_owned(),
                recorded_at_ms: 1,
            },
        )
        .expect("artifact 记录");
        let outbox_path = RemoteNotificationOutboxRuntime::resolve_store_path(directory.path());
        let artifact_path =
            crate::RemoteFeishuAgentReplyArtifactRuntime::resolve_store_path(directory.path());
        RemoteNotificationOutboxRuntime::append_record(&outbox_path, &outbox).expect("写入 outbox");
        crate::RemoteFeishuAgentReplyArtifactRuntime::store_record(&artifact_path, &artifact)
            .expect("写入 artifact");

        assert_eq!(
            remove_expired_state_records(directory.path()).expect("执行维护"),
            2
        );
        assert!(
            RemoteNotificationOutboxRuntime::read_records(&outbox_path)
                .expect("读取 outbox")
                .is_empty()
        );
        assert!(
            crate::RemoteFeishuAgentReplyArtifactRuntime::read_records(&artifact_path)
                .expect("读取 artifact")
                .is_empty()
        );
    }
}
