//! Feishu processing feedback 独立执行器。
//!
//! 主 delivery worker 只负责投递消息并落 feedback task，reaction 生命周期由本模块
//! 独立消费，避免主发送链被 processing 网络波动拖慢。

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use axon_domain::{AxonError, AxonResult, SessionId};
use axon_time::unix_now_ms;

use crate::{RemoteFeishuProcessingOutcome, RemoteFeishuProcessingPlan};

#[path = "remote_feishu_feedback_store.rs"]
mod remote_feishu_feedback_store;
pub use remote_feishu_feedback_store::RemoteFeishuFeedbackTaskStoreRuntime;

/// feedback task 持久化文件名。
pub const REMOTE_FEISHU_FEEDBACK_TASK_FILE: &str = "remote_feishu_feedback_tasks.sqlite3";
/// feedback task 最大重试次数。
pub const REMOTE_FEISHU_FEEDBACK_MAX_ATTEMPTS: u8 = 3;
/// retryable 失败的默认退避时间。
pub const REMOTE_FEISHU_FEEDBACK_RETRY_DELAY_MS: u64 = 60_000;
/// feedback 常驻 worker 默认轮询间隔。
pub const REMOTE_FEISHU_FEEDBACK_BACKGROUND_INTERVAL_MS: u64 = 1_000;
/// background worker 默认全局消费所有 session 的 task。
pub const REMOTE_FEISHU_FEEDBACK_ALL_SESSIONS: u64 = 0;

/// feedback task 状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteFeishuFeedbackTaskStatus {
    /// 等待 feedback worker 执行。
    Pending,
    /// 当前失败但允许重试。
    FailedRetryable,
    /// 已执行完成。
    Completed,
    /// 达到上限后转为永久失败。
    FailedPermanent,
}

/// 单条 feedback task 记录。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFeishuFeedbackTaskRecord {
    /// schema 版本。
    pub schema_version: u32,
    /// task 唯一 ID。
    pub task_id: String,
    /// 对应 dispatch。
    pub dispatch_id: String,
    /// 目标会话。
    pub session_id: u64,
    /// Feishu app_id。
    pub app_id: String,
    /// app_secret 环境变量名。
    pub app_secret_env: String,
    /// API base URL；为空表示官方默认地址。
    pub api_base_url: String,
    /// Feishu message_id。
    pub message_id: String,
    /// processing 完成结果。
    pub outcome: RemoteFeishuProcessingOutcome,
    /// 当前 task 状态。
    pub status: RemoteFeishuFeedbackTaskStatus,
    /// 已尝试次数。
    pub attempt_count: u8,
    /// 下次可重试时间。
    pub next_retry_after_ms: Option<u64>,
    /// 最近错误摘要。
    pub last_error: Option<String>,
    /// 首次记录时间。
    pub recorded_at_ms: u64,
    /// 最近更新时间。
    pub updated_at_ms: u64,
}

/// processing 清理任务输入。
#[derive(Debug, Clone, Copy)]
pub struct RemoteFeishuProcessingCleanupTaskInput<'a> {
    /// Feishu app_id。
    pub app_id: &'a str,
    /// app_secret 环境变量名。
    pub app_secret_env: &'a str,
    /// 目标会话。
    pub session_id: SessionId,
    /// 对应 dispatch。
    pub dispatch_id: &'a str,
    /// 原始入站消息 ID。
    pub message_id: &'a str,
    /// processing 完成结果。
    pub outcome: RemoteFeishuProcessingOutcome,
    /// API base URL。
    pub api_base_url: Option<&'a str>,
    /// 记录时间。
    pub recorded_at_ms: u64,
}

/// 单次 feedback 执行结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuFeedbackExecutionReport {
    /// task id。
    pub task_id: String,
    /// dispatch id。
    pub dispatch_id: String,
    /// message id。
    pub message_id: String,
    /// 最终状态。
    pub status: RemoteFeishuFeedbackTaskStatus,
    /// 执行得到的 processing plans。
    pub processing_plans: Vec<RemoteFeishuProcessingPlan>,
}

/// 单次 feedback worker tick 报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuFeedbackExecutorTickReport {
    /// 目标会话。
    pub session_id: SessionId,
    /// 本轮规划处理 task 数量。
    pub planned_items: usize,
    /// 实际执行数量。
    pub executed_items: usize,
    /// 执行明细。
    pub executions: Vec<RemoteFeishuFeedbackExecutionReport>,
}

/// feedback worker 输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuFeedbackExecutorTickInput {
    /// task store。
    pub feedback_store_path: PathBuf,
    /// processing state store。
    pub processing_store_path: PathBuf,
    /// 目标会话；为 `0` 时表示消费全部 session。
    pub session_id: SessionId,
    /// 当前时间。
    pub now_ms: u64,
    /// 单轮处理上限。
    pub limit: usize,
}

/// feedback 后台执行输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuFeedbackBackgroundInput {
    /// tick 输入模板。
    pub tick_input: RemoteFeishuFeedbackExecutorTickInput,
    /// 后台轮询间隔。
    pub interval_ms: u64,
}

/// feedback 后台执行 handle。
pub struct RemoteFeishuFeedbackBackgroundWorker {
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    receiver: Receiver<RemoteFeishuFeedbackExecutorTickReport>,
}

/// feedback executor runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuFeedbackExecutorRuntime;

impl RemoteFeishuFeedbackExecutorRuntime {
    /// 执行单次 feedback worker tick。
    pub fn tick<FExecute>(
        input: &RemoteFeishuFeedbackExecutorTickInput,
        mut executor: FExecute,
    ) -> AxonResult<RemoteFeishuFeedbackExecutorTickReport>
    where
        FExecute: FnMut(
            &RemoteFeishuFeedbackTaskRecord,
            &Path,
            u64,
        ) -> AxonResult<Vec<RemoteFeishuProcessingPlan>>,
    {
        if input.limit == 0 || input.now_ms == 0 {
            return Err(AxonError::runtime_failure(
                "Feishu feedback worker tick 输入无效",
            ));
        }
        let planned = RemoteFeishuFeedbackTaskStoreRuntime::pending_records(
            &input.feedback_store_path,
            input.session_id,
            input.now_ms,
            input.limit,
        )?;
        let mut executions = Vec::new();
        for task in &planned {
            match executor(task, &input.processing_store_path, input.now_ms) {
                Ok(processing_plans) => {
                    let completed =
                        RemoteFeishuFeedbackTaskStoreRuntime::completed_record(task, input.now_ms);
                    RemoteFeishuFeedbackTaskStoreRuntime::append_record(
                        &input.feedback_store_path,
                        &completed,
                    )?;
                    executions.push(RemoteFeishuFeedbackExecutionReport {
                        task_id: task.task_id.clone(),
                        dispatch_id: task.dispatch_id.clone(),
                        message_id: task.message_id.clone(),
                        status: completed.status,
                        processing_plans,
                    });
                }
                Err(error) => {
                    let failed = RemoteFeishuFeedbackTaskStoreRuntime::failed_record(
                        task,
                        input.now_ms,
                        format!("{error:?}"),
                    );
                    RemoteFeishuFeedbackTaskStoreRuntime::append_record(
                        &input.feedback_store_path,
                        &failed,
                    )?;
                    executions.push(RemoteFeishuFeedbackExecutionReport {
                        task_id: task.task_id.clone(),
                        dispatch_id: task.dispatch_id.clone(),
                        message_id: task.message_id.clone(),
                        status: failed.status,
                        processing_plans: Vec::new(),
                    });
                }
            }
        }
        Ok(RemoteFeishuFeedbackExecutorTickReport {
            session_id: input.session_id,
            planned_items: planned.len(),
            executed_items: executions.len(),
            executions,
        })
    }

    /// 启动常驻 feedback 后台 worker。
    #[must_use]
    pub fn start_background<FExecute>(
        input: RemoteFeishuFeedbackBackgroundInput,
        mut executor: FExecute,
    ) -> RemoteFeishuFeedbackBackgroundWorker
    where
        FExecute: FnMut(
                &RemoteFeishuFeedbackTaskRecord,
                &Path,
                u64,
            ) -> AxonResult<Vec<RemoteFeishuProcessingPlan>>
            + Send
            + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let worker_running = Arc::clone(&running);
        let (sender, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            while worker_running.load(Ordering::SeqCst) {
                let mut tick_input = input.tick_input.clone();
                tick_input.now_ms = now_ms();
                if let Ok(report) = Self::tick(&tick_input, &mut executor) {
                    let _ = sender.send(report);
                }
                thread::sleep(Duration::from_millis(input.interval_ms.max(1)));
            }
        });
        RemoteFeishuFeedbackBackgroundWorker {
            running,
            thread: Some(thread),
            receiver,
        }
    }
}

impl RemoteFeishuFeedbackBackgroundWorker {
    /// 接收下一次后台 tick 报告。
    pub fn recv_tick(
        &self,
        timeout: Duration,
    ) -> AxonResult<RemoteFeishuFeedbackExecutorTickReport> {
        self.receiver
            .recv_timeout(timeout)
            .map_err(|_| AxonError::runtime_failure("Feishu feedback 后台 worker tick 超时"))
    }

    /// 非阻塞读取下一次后台 tick 报告。
    pub fn try_recv_tick(&self) -> AxonResult<Option<RemoteFeishuFeedbackExecutorTickReport>> {
        match self.receiver.try_recv() {
            Ok(tick) => Ok(Some(tick)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(AxonError::runtime_failure(
                "Feishu feedback 后台 worker 通道已断开",
            )),
        }
    }

    /// 停止后台 worker。
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for RemoteFeishuFeedbackBackgroundWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn now_ms() -> u64 {
    unix_now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RemoteFeishuProcessingRuntime;

    fn task(status: RemoteFeishuFeedbackTaskStatus) -> RemoteFeishuFeedbackTaskRecord {
        RemoteFeishuFeedbackTaskRecord {
            schema_version: 1,
            task_id: "dispatch-1:om_msg".to_owned(),
            dispatch_id: "dispatch-1".to_owned(),
            session_id: 9,
            app_id: "cli_xxx".to_owned(),
            app_secret_env: "FEISHU_APP_SECRET".to_owned(),
            api_base_url: "https://open.feishu.cn".to_owned(),
            message_id: "om_msg".to_owned(),
            outcome: RemoteFeishuProcessingOutcome::Success,
            status,
            attempt_count: 0,
            next_retry_after_ms: None,
            last_error: None,
            recorded_at_ms: 100,
            updated_at_ms: 100,
        }
    }

    #[test]
    fn feedback_task_store_plans_pending_items() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-feedback-pending-{}",
            std::process::id()
        ));
        let path = RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&root);
        RemoteFeishuFeedbackTaskStoreRuntime::append_record(
            &path,
            &task(RemoteFeishuFeedbackTaskStatus::Pending),
        )
        .expect("append pending task");
        let tasks =
            RemoteFeishuFeedbackTaskStoreRuntime::pending_records(&path, SessionId(9), 200, 4)
                .expect("query pending task");

        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].message_id, "om_msg");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn retryable_delivery_does_not_enqueue_processing_cleanup() {
        let dispatch = crate::RemoteFeishuWebSocketDispatchPlan {
            adapter: crate::RemoteNotificationAdapterKind::FeishuWebSocket,
            app_id: "cli_xxx".to_owned(),
            app_secret_env: "FEISHU_APP_SECRET".to_owned(),
            websocket_url: "wss://open.feishu.cn/open-apis/ws/v2".to_owned(),
            artifact: crate::RemoteNotificationRenderArtifact {
                adapter: crate::RemoteNotificationAdapterKind::FeishuWebSocket,
                transport: crate::RemoteTransportKind::Feishu,
                content_type: "application/json".to_owned(),
                payload: "{}".to_owned(),
                network_send_allowed: true,
            },
            notification_sending_enabled: true,
            reasons: Vec::new(),
        };
        let execution = crate::RemoteFeishuDeliveryExecutionReport {
            session_id: SessionId(9),
            dispatch_id: "dispatch-retry".to_owned(),
            network_send_attempted: true,
            token_obtained: true,
            adapter_message_id: None,
            send_status: crate::RemoteNotificationAdapterSendStatus::RetryableFailure,
            feedback: crate::RemoteNotificationAdapterSendReport {
                session_id: SessionId(9),
                transport: crate::RemoteTransportKind::Feishu,
                records: Vec::new(),
                receipts: Vec::new(),
                delivered_count: 0,
                retryable_failure_count: 1,
                permanent_failure_count: 0,
                local_only: false,
                network_send_allowed: true,
                raw_payload_forward_allowed: false,
                reasons: Vec::new(),
            },
            reasons: Vec::new(),
        };

        let task = super::RemoteFeishuFeedbackTaskStoreRuntime::task_from_delivery(
            &dispatch,
            &execution,
            Some("om_input"),
            None,
            100,
        );

        assert!(task.is_none());
    }

    #[test]
    fn feedback_executor_tick_completes_task() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-feedback-{}", std::process::id()));
        let feedback_path = RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&root);
        let processing_path =
            crate::RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(&root);
        RemoteFeishuFeedbackTaskStoreRuntime::append_record(
            &feedback_path,
            &task(RemoteFeishuFeedbackTaskStatus::Pending),
        )
        .expect("append");

        let report = RemoteFeishuFeedbackExecutorRuntime::tick(
            &RemoteFeishuFeedbackExecutorTickInput {
                feedback_store_path: feedback_path.clone(),
                processing_store_path: processing_path,
                session_id: SessionId(9),
                now_ms: 200,
                limit: 4,
            },
            |task, _processing_store_path, _now_ms| {
                Ok(vec![
                    RemoteFeishuProcessingRuntime::plan_start(&task.message_id)?,
                    RemoteFeishuProcessingRuntime::plan_complete(&task.message_id, task.outcome)?,
                ])
            },
        )
        .expect("tick");

        assert_eq!(report.planned_items, 1);
        assert_eq!(report.executed_items, 1);
        assert_eq!(
            report.executions[0].status,
            RemoteFeishuFeedbackTaskStatus::Completed
        );
        let records =
            RemoteFeishuFeedbackTaskStoreRuntime::read_records(&feedback_path).expect("read");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, RemoteFeishuFeedbackTaskStatus::Completed);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn feedback_background_worker_emits_tick() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-feishu-feedback-bg-{}", std::process::id()));
        let feedback_path = RemoteFeishuFeedbackTaskStoreRuntime::resolve_store_path(&root);
        let processing_path =
            crate::RemoteFeishuProcessingStateStoreRuntime::resolve_store_path(&root);
        RemoteFeishuFeedbackTaskStoreRuntime::append_record(
            &feedback_path,
            &task(RemoteFeishuFeedbackTaskStatus::Pending),
        )
        .expect("append");
        let mut worker = RemoteFeishuFeedbackExecutorRuntime::start_background(
            RemoteFeishuFeedbackBackgroundInput {
                tick_input: RemoteFeishuFeedbackExecutorTickInput {
                    feedback_store_path: feedback_path,
                    processing_store_path: processing_path,
                    session_id: SessionId(9),
                    now_ms: 1,
                    limit: 4,
                },
                interval_ms: 10,
            },
            |task, _processing_store_path, _now_ms| {
                Ok(vec![RemoteFeishuProcessingRuntime::plan_start(
                    &task.message_id,
                )?])
            },
        );

        let tick = worker.recv_tick(Duration::from_millis(500)).expect("tick");
        assert_eq!(tick.executed_items, 1);
        worker.stop();
        let _ = std::fs::remove_dir_all(root);
    }
}
