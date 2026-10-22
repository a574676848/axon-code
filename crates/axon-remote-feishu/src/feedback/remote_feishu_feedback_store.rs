use std::path::{Path, PathBuf};

use axon_domain::{AxonError, AxonResult, SessionId};
use axon_remote_runtime::{RemoteStateStoreIndex, RemoteStateStoreRuntime};

use crate::remote_feishu::RemoteFeishuDeliveryExecutionReport;
use crate::{
    REMOTE_FEISHU_FEEDBACK_ALL_SESSIONS, REMOTE_FEISHU_FEEDBACK_MAX_ATTEMPTS,
    REMOTE_FEISHU_FEEDBACK_RETRY_DELAY_MS, REMOTE_FEISHU_FEEDBACK_TASK_FILE,
    RemoteFeishuFeedbackTaskRecord, RemoteFeishuFeedbackTaskStatus,
    RemoteFeishuProcessingCleanupTaskInput, RemoteFeishuProcessingOutcome,
};

/// feedback task store runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuFeedbackTaskStoreRuntime;

const FEEDBACK_STATE_NAMESPACE: &str = "feishu_feedback";
const FEEDBACK_STATUS_PENDING: &str = "pending";
const FEEDBACK_STATUS_RETRYABLE: &str = "failed_retryable";
const FEEDBACK_STATUS_COMPLETED: &str = "completed";
const FEEDBACK_STATUS_PERMANENT: &str = "failed_permanent";

impl RemoteFeishuFeedbackTaskStoreRuntime {
    /// 解析 task store 路径。
    #[must_use]
    pub fn resolve_store_path(root: impl AsRef<Path>) -> PathBuf {
        root.as_ref().join(REMOTE_FEISHU_FEEDBACK_TASK_FILE)
    }

    /// 由最终 delivery 执行结果构造 processing 清理任务。
    #[must_use]
    pub fn task_from_delivery(
        dispatch_plan: &crate::RemoteFeishuWebSocketDispatchPlan,
        execution: &RemoteFeishuDeliveryExecutionReport,
        processing_message_id: Option<&str>,
        api_base_url: Option<&str>,
        recorded_at_ms: u64,
    ) -> Option<RemoteFeishuFeedbackTaskRecord> {
        let message_id = processing_message_id?;
        let outcome = match execution.send_status {
            crate::RemoteNotificationAdapterSendStatus::Delivered => {
                RemoteFeishuProcessingOutcome::Success
            }
            crate::RemoteNotificationAdapterSendStatus::PermanentFailure => {
                RemoteFeishuProcessingOutcome::Failure
            }
            crate::RemoteNotificationAdapterSendStatus::RetryableFailure => return None,
        };
        Some(Self::task_from_cleanup(
            RemoteFeishuProcessingCleanupTaskInput {
                app_id: &dispatch_plan.app_id,
                app_secret_env: &dispatch_plan.app_secret_env,
                session_id: execution.session_id,
                dispatch_id: &execution.dispatch_id,
                message_id,
                outcome,
                api_base_url,
                recorded_at_ms,
            },
        ))
    }

    /// 构造 durable processing 清理任务。
    #[must_use]
    pub fn task_from_cleanup(
        input: RemoteFeishuProcessingCleanupTaskInput<'_>,
    ) -> RemoteFeishuFeedbackTaskRecord {
        RemoteFeishuFeedbackTaskRecord {
            schema_version: 1,
            task_id: format!("{}:{}", input.dispatch_id, input.message_id),
            dispatch_id: input.dispatch_id.to_owned(),
            session_id: input.session_id.0,
            app_id: input.app_id.to_owned(),
            app_secret_env: input.app_secret_env.to_owned(),
            api_base_url: input.api_base_url.unwrap_or_default().to_owned(),
            message_id: input.message_id.to_owned(),
            outcome: input.outcome,
            status: RemoteFeishuFeedbackTaskStatus::Pending,
            attempt_count: 0,
            next_retry_after_ms: None,
            last_error: None,
            recorded_at_ms: input.recorded_at_ms,
            updated_at_ms: input.recorded_at_ms,
        }
    }

    /// 追加 task 记录。
    pub fn append_record(
        path: impl AsRef<Path>,
        record: &RemoteFeishuFeedbackTaskRecord,
    ) -> AxonResult<()> {
        validate_record(record)?;
        RemoteStateStoreRuntime::upsert(path, &record.task_id, feedback_index(record), record)
    }

    /// 读取全部 task 记录。
    pub fn read_records(path: impl AsRef<Path>) -> AxonResult<Vec<RemoteFeishuFeedbackTaskRecord>> {
        RemoteStateStoreRuntime::read_all(path)?
            .into_iter()
            .map(|record| {
                validate_record(&record)?;
                Ok(record)
            })
            .collect()
    }

    /// 获取当前可执行的 pending/retryable task。
    pub fn pending_records(
        path: impl AsRef<Path>,
        session_id: SessionId,
        now_ms: u64,
        limit: usize,
    ) -> AxonResult<Vec<RemoteFeishuFeedbackTaskRecord>> {
        let partition = (session_id.0 != REMOTE_FEISHU_FEEDBACK_ALL_SESSIONS)
            .then(|| feedback_partition(session_id.0));
        let mut tasks = RemoteStateStoreRuntime::read_indexed_limited::<
            RemoteFeishuFeedbackTaskRecord,
        >(
            path,
            FEEDBACK_STATE_NAMESPACE,
            partition.as_deref(),
            &[FEEDBACK_STATUS_PENDING, FEEDBACK_STATUS_RETRYABLE],
            limit,
        )?
        .into_iter()
        .filter(|record| {
            session_id.0 == REMOTE_FEISHU_FEEDBACK_ALL_SESSIONS || record.session_id == session_id.0
        })
        .filter(|record| match record.status {
            RemoteFeishuFeedbackTaskStatus::Pending => true,
            RemoteFeishuFeedbackTaskStatus::FailedRetryable => record
                .next_retry_after_ms
                .is_none_or(|retry_at| retry_at <= now_ms),
            RemoteFeishuFeedbackTaskStatus::Completed
            | RemoteFeishuFeedbackTaskStatus::FailedPermanent => false,
        })
        .collect::<Vec<_>>();
        tasks.sort_by_key(|record| record.updated_at_ms);
        tasks.truncate(limit);
        Ok(tasks)
    }

    /// 构造成功完成记录。
    #[must_use]
    pub fn completed_record(
        task: &RemoteFeishuFeedbackTaskRecord,
        updated_at_ms: u64,
    ) -> RemoteFeishuFeedbackTaskRecord {
        RemoteFeishuFeedbackTaskRecord {
            status: RemoteFeishuFeedbackTaskStatus::Completed,
            attempt_count: task.attempt_count.saturating_add(1),
            next_retry_after_ms: None,
            last_error: None,
            updated_at_ms,
            ..task.clone()
        }
    }

    /// 构造失败重试/终态记录。
    #[must_use]
    pub fn failed_record(
        task: &RemoteFeishuFeedbackTaskRecord,
        updated_at_ms: u64,
        error: String,
    ) -> RemoteFeishuFeedbackTaskRecord {
        let next_attempt = task.attempt_count.saturating_add(1);
        let status = if next_attempt >= REMOTE_FEISHU_FEEDBACK_MAX_ATTEMPTS {
            RemoteFeishuFeedbackTaskStatus::FailedPermanent
        } else {
            RemoteFeishuFeedbackTaskStatus::FailedRetryable
        };
        RemoteFeishuFeedbackTaskRecord {
            status,
            attempt_count: next_attempt,
            next_retry_after_ms: (status == RemoteFeishuFeedbackTaskStatus::FailedRetryable)
                .then_some(updated_at_ms.saturating_add(REMOTE_FEISHU_FEEDBACK_RETRY_DELAY_MS)),
            last_error: Some(error),
            updated_at_ms,
            ..task.clone()
        }
    }

    /// 清理保留期之前的终态 feedback task。
    pub fn remove_terminal_records_before(
        path: impl AsRef<Path>,
        cutoff_ms: u64,
    ) -> AxonResult<usize> {
        RemoteStateStoreRuntime::delete_indexed_before(
            path,
            FEEDBACK_STATE_NAMESPACE,
            &[FEEDBACK_STATUS_COMPLETED, FEEDBACK_STATUS_PERMANENT],
            cutoff_ms,
        )
    }

    /// 删除指定 dispatch 关联的 feedback task 当前状态。
    pub fn remove_for_dispatches(
        path: impl AsRef<Path>,
        dispatch_ids: &[String],
    ) -> AxonResult<usize> {
        RemoteStateStoreRuntime::delete_keys(path, dispatch_ids)
    }
}

fn feedback_index(record: &RemoteFeishuFeedbackTaskRecord) -> RemoteStateStoreIndex {
    RemoteStateStoreIndex::new(
        FEEDBACK_STATE_NAMESPACE,
        feedback_partition(record.session_id),
        feedback_status(record.status),
        record.updated_at_ms,
    )
}

fn feedback_partition(session_id: u64) -> String {
    format!("session:{session_id}")
}

const fn feedback_status(status: RemoteFeishuFeedbackTaskStatus) -> &'static str {
    match status {
        RemoteFeishuFeedbackTaskStatus::Pending => FEEDBACK_STATUS_PENDING,
        RemoteFeishuFeedbackTaskStatus::FailedRetryable => FEEDBACK_STATUS_RETRYABLE,
        RemoteFeishuFeedbackTaskStatus::Completed => FEEDBACK_STATUS_COMPLETED,
        RemoteFeishuFeedbackTaskStatus::FailedPermanent => FEEDBACK_STATUS_PERMANENT,
    }
}

fn validate_record(record: &RemoteFeishuFeedbackTaskRecord) -> AxonResult<()> {
    if record.schema_version == 0
        || record.task_id.trim().is_empty()
        || record.dispatch_id.trim().is_empty()
        || record.session_id == 0
        || record.app_id.trim().is_empty()
        || record.app_secret_env.trim().is_empty()
        || record.message_id.trim().is_empty()
        || record.recorded_at_ms == 0
        || record.updated_at_ms == 0
    {
        return Err(AxonError::runtime_failure("Feishu feedback task 记录无效"));
    }
    Ok(())
}
