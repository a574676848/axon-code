//! Feishu 远控 worker 化执行运行时。
//!
//! 该模块把 outbox pending -> dispatch batch -> delivery execute -> feedback append
//! 串成单次 worker tick，作为超越 CLI 手工执行的主链基础。

use std::path::Path;

use axon_domain::{AxonError, AxonResult, SessionId};

use crate::remote_feishu::RemoteFeishuDeliveryExecutionReport;
use crate::remote_feishu_scheduler::REMOTE_FEISHU_CHAT_LEASE_DURATION_MS;
use crate::{
    RemoteFeishuFeedbackTaskRecord, RemoteFeishuFeedbackTaskStoreRuntime,
    RemoteFeishuProcessingOutcome, RemoteFeishuProcessingPlan, RemoteFeishuProcessingRuntime,
    RemoteFeishuProcessingStateStoreRuntime, RemoteFeishuSchedulerStoreRuntime,
    RemoteFeishuWebSocketDispatchPlan, RemoteNotificationAdapterDispatchRuntime,
    RemoteNotificationOutboxRuntime, RemoteTransportKind,
};
use axon_remote_runtime::REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS;

const FEEDBACK_PERSIST_ATTEMPTS: usize = 3;

/// 单次 Feishu worker tick 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuExecutorTickReport {
    /// session。
    pub session_id: SessionId,
    /// 计划出的 dispatch 数量。
    pub planned_items: usize,
    /// 实际执行数量。
    pub executed_items: usize,
    /// 单次执行明细。
    pub executions: Vec<RemoteFeishuDeliveryExecutionReport>,
    /// processing 动作计划。
    pub processing_plans: Vec<RemoteFeishuProcessingPlan>,
    /// 同 chat 被延后的 dispatch 数量。
    pub deferred_items: usize,
    /// 已入队的 feedback task 数量。
    pub enqueued_feedback_tasks: usize,
}

/// Feishu worker 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuExecutorTickInput {
    /// outbox store。
    pub outbox_store_path: std::path::PathBuf,
    /// processing state store。
    pub processing_store_path: std::path::PathBuf,
    /// scheduler state store。
    pub scheduler_store_path: std::path::PathBuf,
    /// feedback task store。
    pub feedback_store_path: std::path::PathBuf,
    /// 目标 session。
    pub session_id: SessionId,
    /// worker 当前时间。
    pub now_ms: u64,
    /// ack 时间。
    pub accepted_at_ms: u64,
    /// 单次最多处理多少条。
    pub limit: usize,
    /// 当前 Daemon host 的 fencing owner identity。
    pub attempt_owner: String,
}

/// Feishu worker runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuExecutorRuntime;

impl RemoteFeishuExecutorRuntime {
    /// 执行单次 worker tick。
    pub fn tick<FDispatch, FExecute, FFeedback>(
        input: &RemoteFeishuExecutorTickInput,
        mut dispatch_builder: FDispatch,
        mut executor: FExecute,
        mut feedback_builder: FFeedback,
    ) -> AxonResult<RemoteFeishuExecutorTickReport>
    where
        FDispatch: FnMut(
            &crate::RemoteNotificationOutboxRecord,
        ) -> AxonResult<RemoteFeishuWebSocketDispatchPlan>,
        FExecute: FnMut(
            &RemoteFeishuWebSocketDispatchPlan,
            &crate::RemoteNotificationOutboxRecord,
            u64,
        ) -> AxonResult<RemoteFeishuDeliveryExecutionReport>,
        FFeedback: FnMut(
            &RemoteFeishuWebSocketDispatchPlan,
            &crate::RemoteNotificationOutboxRecord,
            &RemoteFeishuDeliveryExecutionReport,
            u64,
        ) -> AxonResult<Option<RemoteFeishuFeedbackTaskRecord>>,
    {
        if input.session_id.0 == 0
            || input.limit == 0
            || input.accepted_at_ms < input.now_ms
            || input.attempt_owner.trim().is_empty()
        {
            return Err(AxonError::runtime_failure("Feishu worker tick 输入无效"));
        }
        RemoteNotificationOutboxRuntime::recover_stale_attempts(
            &input.outbox_store_path,
            input.now_ms,
            REMOTE_FEISHU_CHAT_LEASE_DURATION_MS,
        )?;
        let batch = RemoteNotificationAdapterDispatchRuntime::plan_from_outbox(
            &input.outbox_store_path,
            input.session_id,
            RemoteTransportKind::Feishu,
            input.now_ms,
            input.accepted_at_ms,
            input.limit,
        )?;
        let schedule = RemoteFeishuSchedulerStoreRuntime::claim_items(
            &input.scheduler_store_path,
            &batch.items,
            input.now_ms,
            input.accepted_at_ms,
        )?;
        let execution_result = (|| {
            let mut executions = Vec::new();
            let mut enqueued_feedback_tasks = 0usize;
            for item in &schedule.active_items {
                let record = RemoteNotificationOutboxRuntime::mark_attempting(
                    &input.outbox_store_path,
                    &item.dispatch_id,
                    input.accepted_at_ms,
                    &input.attempt_owner,
                )?;
                let dispatch_plan = match dispatch_builder(&record) {
                    Ok(plan) => plan,
                    Err(error) => {
                        append_failed_record(
                            &input.outbox_store_path,
                            &record,
                            input.accepted_at_ms,
                        )?;
                        return Err(error);
                    }
                };
                let execution = match executor(
                    &dispatch_plan,
                    &record,
                    input.accepted_at_ms.saturating_add(1),
                ) {
                    Ok(execution) => execution,
                    Err(error) => {
                        append_failed_record(
                            &input.outbox_store_path,
                            &record,
                            input.accepted_at_ms,
                        )?;
                        return Err(error);
                    }
                };
                // 网络结果先进入 durable outbox；feedback task 是可补偿的后处理，
                // 不得因其失败把已送达消息重新暴露给重试。
                append_feedback_records(&input.outbox_store_path, &execution.feedback.records)?;
                if let Some(task) = feedback_builder(
                    &dispatch_plan,
                    &record,
                    &execution,
                    input.accepted_at_ms.saturating_add(2),
                )? {
                    RemoteFeishuFeedbackTaskStoreRuntime::append_record(
                        &input.feedback_store_path,
                        &task,
                    )?;
                    enqueued_feedback_tasks = enqueued_feedback_tasks.saturating_add(1);
                }
                executions.push(execution);
            }
            Ok((executions, enqueued_feedback_tasks))
        })();
        release_claimed_items(
            &input.scheduler_store_path,
            &schedule.active_items,
            input.accepted_at_ms.saturating_add(1),
        )?;
        let (executions, enqueued_feedback_tasks) = execution_result?;
        let processing_plans = Vec::new();

        Ok(RemoteFeishuExecutorTickReport {
            session_id: input.session_id,
            planned_items: batch.items.len(),
            executed_items: executions.len(),
            executions,
            processing_plans,
            deferred_items: schedule.deferred_items.len(),
            enqueued_feedback_tasks,
        })
    }
}

fn release_claimed_items(
    scheduler_store_path: &Path,
    items: &[crate::RemoteNotificationAdapterDispatchItem],
    recorded_at_ms: u64,
) -> AxonResult<()> {
    for item in items {
        let release = RemoteFeishuSchedulerStoreRuntime::release_record(item, recorded_at_ms)?;
        RemoteFeishuSchedulerStoreRuntime::append_record(scheduler_store_path, &release)?;
    }
    Ok(())
}

fn append_failed_record(
    outbox_store_path: &Path,
    source: &crate::RemoteNotificationOutboxRecord,
    failed_at_ms: u64,
) -> AxonResult<()> {
    let mut failed = source.clone();
    failed.updated_at_ms = failed_at_ms.max(source.updated_at_ms.saturating_add(1));
    failed.attempt_count = source.attempt_count.saturating_add(1);
    failed.status = if failed.attempt_count >= REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS {
        crate::RemoteNotificationDispatchStatus::Abandoned
    } else {
        crate::RemoteNotificationDispatchStatus::Failed
    };
    failed.next_retry_after_ms = (failed.status == crate::RemoteNotificationDispatchStatus::Failed)
        .then_some(failed.updated_at_ms.saturating_add(60_000));
    failed.last_error = Some("Feishu delivery execution failed".to_owned());
    failed.attempt_started_at_ms = None;
    failed.attempt_owner = None;
    failed.recovery_marker = None;
    "remote_feishu:delivery_failed".clone_into(&mut failed.reason);
    RemoteNotificationOutboxRuntime::append_record(outbox_store_path, &failed).map(|_| ())
}

/// 基于真实 Feishu 网络执行 processing start/complete，并持久化 reaction 状态。
pub fn process_processing_state(
    dispatch_plan: &RemoteFeishuWebSocketDispatchPlan,
    processing_store_path: &Path,
    message_id: &str,
    send_status: crate::RemoteNotificationAdapterSendStatus,
    accepted_at_ms: u64,
) -> AxonResult<Vec<RemoteFeishuProcessingPlan>> {
    let start = RemoteFeishuProcessingRuntime::execute_start(
        &dispatch_plan.app_id,
        &dispatch_plan.app_secret_env,
        Some(crate::REMOTE_FEISHU_API_BASE_URL),
        message_id,
        accepted_at_ms,
    )?;
    RemoteFeishuProcessingStateStoreRuntime::append_record(processing_store_path, &start)?;
    let start_plan = RemoteFeishuProcessingRuntime::plan_start(message_id)?;
    let active_state = RemoteFeishuProcessingStateStoreRuntime::active_reaction(
        processing_store_path,
        message_id,
    )?;
    let completion_outcome = if send_status == crate::RemoteNotificationAdapterSendStatus::Delivered
    {
        RemoteFeishuProcessingOutcome::Success
    } else {
        RemoteFeishuProcessingOutcome::Failure
    };
    let completion_states = RemoteFeishuProcessingRuntime::execute_complete(
        &dispatch_plan.app_id,
        &dispatch_plan.app_secret_env,
        Some(crate::REMOTE_FEISHU_API_BASE_URL),
        message_id,
        completion_outcome,
        active_state.as_ref(),
        accepted_at_ms.saturating_add(1),
    )?;
    for state in &completion_states {
        RemoteFeishuProcessingStateStoreRuntime::append_record(processing_store_path, state)?;
    }
    let completion_plan =
        RemoteFeishuProcessingRuntime::plan_complete(message_id, completion_outcome)?;
    Ok(vec![start_plan, completion_plan])
}

fn append_feedback_records(
    outbox_store_path: &Path,
    feedback_records: &[crate::RemoteNotificationOutboxRecord],
) -> AxonResult<()> {
    for feedback_record in feedback_records {
        let mut last_error = None;
        for _ in 0..FEEDBACK_PERSIST_ATTEMPTS {
            match RemoteNotificationOutboxRuntime::append_record(outbox_store_path, feedback_record)
            {
                Ok(_) => {
                    last_error = None;
                    break;
                }
                Err(error) => last_error = Some(error),
            }
        }
        if let Some(error) = last_error {
            if feedback_record.status == crate::RemoteNotificationDispatchStatus::Delivered {
                axon_logger::warning(&format!(
                    "Feishu 网络发送已成功但 outbox 回执落盘失败，恢复时将使用同一 dispatch_id={} 作为 uuid 幂等键：{error:?}",
                    feedback_record.dispatch_id
                ));
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        RemoteFeishuDeliveryRuntime, RemoteFeishuHttpResponse, RemoteNotificationAdapterKind,
        RemoteNotificationDelivery, RemoteNotificationFanoutPlan, RemoteNotificationKind,
        RemoteNotificationOutboxInput, RemoteNotificationRenderArtifact,
        RemoteNotificationSeverity, WireLifecycleStatus,
    };

    fn seed_outbox(path: &Path) -> crate::RemoteNotificationOutboxRecord {
        let record =
            crate::RemoteNotificationOutboxRecord::from_input(RemoteNotificationOutboxInput {
                dispatch_id: "dispatch-1".to_owned(),
                plan: RemoteNotificationFanoutPlan {
                    session_id: SessionId(9),
                    subscriber_id: "sub".to_owned(),
                    connection_id: "oc_chat_1".to_owned(),
                    pairing_id: "pairing-1".to_owned(),
                    transport: RemoteTransportKind::Feishu,
                    subscribed_kinds: vec![RemoteNotificationKind::TaskStarted],
                    delivered: vec![RemoteNotificationDelivery {
                        kind: RemoteNotificationKind::TaskStarted,
                        wire_event: crate::remote_notification_wire_lifecycle_event(
                            SessionId(9),
                            "remote-feishu-executor-test",
                            WireLifecycleStatus::Waiting,
                            "远控任务开始",
                            None,
                            100,
                        ),
                        bounded_body: "任务开始".to_owned(),
                        reference_ids: vec!["thread:9".to_owned()],
                        local_action_hint: "本机打开线程".to_owned(),
                        severity: RemoteNotificationSeverity::ActionRequired,
                    }],
                    dropped_count: 0,
                    local_only: true,
                    network_send_allowed: false,
                    raw_payload_forward_allowed: false,
                },
                status: crate::RemoteNotificationDispatchStatus::Pending,
                recorded_at_ms: 100,
                updated_at_ms: 100,
                attempt_count: 0,
                next_retry_after_ms: None,
                last_error: None,
                adapter_message_id: None,
                source_trace: "test".to_owned(),
                reason: "planned".to_owned(),
            })
            .expect("record");
        crate::RemoteNotificationOutboxRuntime::append_record(path, &record).expect("append");
        record
    }

    #[test]
    fn feishu_executor_enqueues_original_message_cleanup_after_delivery() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-worker-{}", std::process::id()));
        let path = crate::RemoteNotificationOutboxRuntime::resolve_store_path(&root);
        let record = seed_outbox(&path);
        let report = RemoteFeishuExecutorRuntime::tick(
            &RemoteFeishuExecutorTickInput {
                outbox_store_path: path.clone(),
                processing_store_path: RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(
                    &root,
                ),
                scheduler_store_path: RemoteFeishuSchedulerStoreRuntime::resolve_store_path(&root),
                feedback_store_path: RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(
                    &root,
                ),
                session_id: SessionId(9),
                now_ms: 100,
                accepted_at_ms: 200,
                limit: 4,
                attempt_owner: "test-owner".to_owned(),
            },
            |_record| Ok(test_dispatch_plan()),
            |_dispatch, accepted, sent_at_ms| {
                RemoteFeishuDeliveryRuntime::execute_with_http_and_secret(
                    &test_dispatch_plan(),
                    "secret-value",
                    Some("https://open.feishu.cn"),
                    accepted,
                    sent_at_ms,
                    fake_feishu_http_response,
                )
            },
            |dispatch_plan, _record, execution, accepted_at_ms| {
                Ok(RemoteFeishuFeedbackTaskStoreRuntime::task_from_delivery(
                    dispatch_plan,
                    execution,
                    Some("om_input"),
                    Some("https://open.feishu.cn"),
                    accepted_at_ms,
                ))
            },
        )
        .expect("tick");

        assert_worker_report(&report, &record);
        assert_delivered_outbox(&path);
        assert_feedback_task(&root);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dispatch_plan_failure_keeps_unstarted_batch_items_ready_for_the_next_tick() {
        let directory = axon_test_support::test_temp_dir();
        let root = directory.path();
        let outbox_path = crate::RemoteNotificationOutboxRuntime::resolve_store_path(root);
        let scheduler_path = RemoteFeishuSchedulerStoreRuntime::resolve_store_path(root);
        let first = seed_outbox(&outbox_path);
        let mut second = first.clone();
        second.dispatch_id = "dispatch-2".to_owned();
        crate::RemoteNotificationOutboxRuntime::append_record(&outbox_path, &second)
            .expect("append second");

        let result = RemoteFeishuExecutorRuntime::tick(
            &RemoteFeishuExecutorTickInput {
                outbox_store_path: outbox_path.clone(),
                processing_store_path: RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(
                    root,
                ),
                scheduler_store_path: scheduler_path,
                feedback_store_path: RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(root),
                session_id: SessionId(9),
                now_ms: 100,
                accepted_at_ms: 200,
                limit: 4,
                attempt_owner: "test-owner".to_owned(),
            },
            |_record| Err(AxonError::runtime_failure("dispatch plan failure")),
            |_dispatch, _record, _sent_at_ms| unreachable!("构建失败前不能执行网络发送"),
            |_dispatch, _record, _execution, _accepted_at_ms| {
                unreachable!("构建失败前不能创建 feedback task")
            },
        );

        assert!(result.is_err());
        let records = crate::RemoteNotificationOutboxRuntime::latest_records(&outbox_path)
            .expect("latest records");
        let first = records
            .iter()
            .find(|record| record.dispatch_id == "dispatch-1")
            .expect("first record");
        let second = records
            .iter()
            .find(|record| record.dispatch_id == "dispatch-2")
            .expect("second record");
        assert_eq!(
            first.status,
            crate::RemoteNotificationDispatchStatus::Failed
        );
        assert_eq!(
            second.status,
            crate::RemoteNotificationDispatchStatus::Pending
        );
    }

    #[test]
    fn exhausted_delivery_attempts_are_abandoned_instead_of_exceeding_the_ledger_limit() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-exhausted-attempts-{}",
            std::process::id()
        ));
        let path = crate::RemoteNotificationOutboxRuntime::resolve_store_path(&root);
        let mut record = seed_outbox(&path);
        record.attempt_count = REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS.saturating_sub(1);

        append_failed_record(&path, &record, 200).expect("append exhausted failure");
        let latest = crate::RemoteNotificationOutboxRuntime::latest_records(&path).expect("latest");

        assert_eq!(latest.len(), 1);
        assert_eq!(
            latest[0].status,
            crate::RemoteNotificationDispatchStatus::Abandoned
        );
        assert_eq!(
            latest[0].attempt_count,
            REMOTE_NOTIFICATION_OUTBOX_MAX_ATTEMPTS
        );
        assert_eq!(latest[0].next_retry_after_ms, None);
        let _ = std::fs::remove_dir_all(root);
    }

    fn test_dispatch_plan() -> RemoteFeishuWebSocketDispatchPlan {
        RemoteFeishuWebSocketDispatchPlan {
            adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
            app_id: "cli_xxx".to_owned(),
            app_secret_env: "FEISHU_APP_SECRET".to_owned(),
            websocket_url: "wss://open.feishu.cn/open-apis/ws/v2".to_owned(),
            artifact: RemoteNotificationRenderArtifact {
                adapter: RemoteNotificationAdapterKind::FeishuWebSocket,
                transport: RemoteTransportKind::Feishu,
                content_type: "application/json".to_owned(),
                payload: "{\"msg\":\"card\"}".to_owned(),
                network_send_allowed: false,
            },
            notification_sending_enabled: true,
            reasons: vec![],
        }
    }

    fn fake_feishu_http_response(
        request: &crate::RemoteFeishuHttpRequestPlan,
    ) -> AxonResult<RemoteFeishuHttpResponse> {
        if request.url.contains("tenant_access_token") {
            return Ok(RemoteFeishuHttpResponse {
                status: 200,
                body: r#"{"code":0,"tenant_access_token":"tenant-token"}"#.to_owned(),
            });
        }
        Ok(RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"message_id":"om_worker"}}"#.to_owned(),
        })
    }

    fn assert_worker_report(
        report: &RemoteFeishuExecutorTickReport,
        record: &crate::RemoteNotificationOutboxRecord,
    ) {
        assert_eq!(report.planned_items, 1);
        assert_eq!(report.executed_items, 1);
        assert_eq!(report.executions[0].dispatch_id, record.dispatch_id);
        assert_eq!(report.processing_plans.len(), 0);
        assert_eq!(report.deferred_items, 0);
        assert_eq!(report.enqueued_feedback_tasks, 1);
    }

    fn assert_delivered_outbox(path: &Path) {
        let latest = crate::RemoteNotificationOutboxRuntime::latest_records(path).expect("latest");
        assert_eq!(latest.len(), 1);
        assert_eq!(
            latest[0].status,
            crate::RemoteNotificationDispatchStatus::Delivered
        );
    }

    fn assert_feedback_task(root: &Path) {
        let feedback_records = RemoteFeishuFeedbackTaskStoreRuntime::read_records(
            RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(root),
        )
        .expect("feedback records");
        assert_eq!(feedback_records.len(), 1);
        assert_eq!(feedback_records[0].message_id, "om_input");
        assert_eq!(
            feedback_records[0].outcome,
            crate::RemoteFeishuProcessingOutcome::Success
        );
    }
}
