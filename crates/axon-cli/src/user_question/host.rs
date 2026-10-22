//! 宿主问答注册表与有界等待原语（UQ-003）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axon_agent_runtime::{
    QuestionWaitBudget, QuestionWaitResult, RunCancelToken, UserQuestionPort,
};
use axon_domain::{
    DEFAULT_SUSPENDED_HANDOFF, UserAnswerItem, UserQuestionItem, UserQuestionRecord,
    UserQuestionState,
};
use axon_session::UserQuestionStore;

/// 用户作答交付载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswerDelivery {
    /// 作答的 JSON 字符串。
    pub answer_json: String,
    /// 来源渠道。
    pub channel: Option<String>,
}

/// 宿主问答事件发射器接口。
pub trait UserQuestionEventSink: Send + Sync {
    /// 发射问答状态变化事件（pending / extended / settled / suspended）。
    ///
    /// `lane_id` 是会话车道身份（Daemon 下等于主 turn 的会话 ID）：事件按它路由到
    /// 当前正在运行的那一轮，同时也作为 wire 事件的会话身份。
    fn emit_question_event(
        &self,
        lane_id: &str,
        part_id: &str,
        call_id: &str,
        status: &str,
        questions: &[UserQuestionItem],
        answers: &[UserAnswerItem],
        handoff: Option<&str>,
        deadline_at_ms: i64,
    );
}

/// 空事件发射器。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopUserQuestionEventSink;

impl UserQuestionEventSink for NoopUserQuestionEventSink {
    fn emit_question_event(
        &self,
        _lane_id: &str,
        _part_id: &str,
        _call_id: &str,
        _status: &str,
        _questions: &[UserQuestionItem],
        _answers: &[UserAnswerItem],
        _handoff: Option<&str>,
        _deadline_at_ms: i64,
    ) {
    }
}

type WaiterTx = tokio::sync::oneshot::Sender<QuestionAnswerDelivery>;

/// RAII 注销守护：离开作用域（含 panic）必定从注册表中移除。
struct WaiterGuard {
    call_id: String,
    waiters: Arc<Mutex<HashMap<String, WaiterTx>>>,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        if let Ok(mut lock) = self.waiters.lock() {
            lock.remove(&self.call_id);
        }
    }
}

/// 宿主用户问答原语实现。
#[derive(Clone)]
pub struct UserQuestionHost {
    store: Arc<dyn UserQuestionStore>,
    waiters: Arc<Mutex<HashMap<String, WaiterTx>>>,
    sink: Arc<dyn UserQuestionEventSink>,
    shutdown_token: Option<RunCancelToken>,
}

impl UserQuestionHost {
    /// 创建宿主实例。
    pub fn new(
        store: Arc<dyn UserQuestionStore>,
        sink: Arc<dyn UserQuestionEventSink>,
        shutdown_token: Option<RunCancelToken>,
    ) -> Self {
        Self {
            store,
            waiters: Arc::new(Mutex::new(HashMap::new())),
            sink,
            shutdown_token,
        }
    }

    /// 尝试向活等待者投递答案；若存在且投递成功返回 true。
    pub fn deliver_to_waiter(&self, call_id: &str, delivery: QuestionAnswerDelivery) -> bool {
        let tx = {
            let mut lock = match self.waiters.lock() {
                Ok(l) => l,
                Err(poison) => poison.into_inner(),
            };
            lock.remove(call_id)
        };
        if let Some(tx) = tx {
            tx.send(delivery).is_ok()
        } else {
            false
        }
    }

    /// 查询当前注册的等待者数量。
    pub fn waiter_count(&self) -> usize {
        let lock = match self.waiters.lock() {
            Ok(l) => l,
            Err(poison) => poison.into_inner(),
        };
        lock.len()
    }
}

impl UserQuestionPort for UserQuestionHost {
    fn wait_for_answer(
        &self,
        record: &UserQuestionRecord,
        budget: QuestionWaitBudget,
        cancel_token: &RunCancelToken,
    ) -> Result<QuestionWaitResult, String> {
        let (tx, mut rx) = tokio::sync::oneshot::channel();

        // 1. 注册等待者并建立 RAII 守护
        {
            let mut lock = self
                .waiters
                .lock()
                .map_err(|_| "注册表互斥锁已中毒".to_string())?;
            lock.insert(record.tool_call_id.clone(), tx);
        }
        let _guard = WaiterGuard {
            call_id: record.tool_call_id.clone(),
            waiters: self.waiters.clone(),
        };

        // 2. 落库并发出初始 pending 事件
        let actual_record = self
            .store
            .insert_question(record)
            .map_err(|e| format!("提问记录落库失败: {e}"))?;

        let questions: Vec<UserQuestionItem> =
            serde_json::from_str(&actual_record.questions_json).unwrap_or_default();
        let part_id = crate::run_commands::question_part_id(&actual_record.tool_call_id);

        self.sink.emit_question_event(
            &actual_record.lane_id,
            &part_id,
            &actual_record.tool_call_id,
            "pending",
            &questions,
            &[],
            actual_record.handoff_text.as_deref(),
            actual_record.deadline_at_ms,
        );

        // 3. 有界等待循环
        let mut current_window_ms = budget.initial_window_ms;
        let mut elapsed_in_window_ms = 0_u64;
        let mut total_elapsed_ms = 0_u64;
        let mut current_deadline_at_ms = actual_record.deadline_at_ms;

        let poll_interval = Duration::from_millis(budget.poll_interval_ms);

        loop {
            // 检查外部取消或宿主关停信号
            if cancel_token.is_cancelled()
                || self
                    .shutdown_token
                    .as_ref()
                    .is_some_and(|t| t.is_cancelled())
            {
                let _ = self.store.cas_update_state(
                    &actual_record.question_id,
                    UserQuestionState::Pending,
                    UserQuestionState::Cancelled,
                    None,
                    None,
                );
                return Ok(QuestionWaitResult::Aborted {
                    reason: "ASK_ABORTED".to_string(),
                });
            }

            // 检查是否在 channel 中收到作答
            match rx.try_recv() {
                Ok(delivery) => {
                    return Ok(QuestionWaitResult::Answered {
                        answer_json: delivery.answer_json,
                        channel: delivery.channel,
                    });
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    return Ok(QuestionWaitResult::Aborted {
                        reason: "ASK_ABORTED".to_string(),
                    });
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }

            // 休眠步长检查取消
            if cancel_token.wait_timeout(poll_interval) {
                let _ = self.store.cas_update_state(
                    &actual_record.question_id,
                    UserQuestionState::Pending,
                    UserQuestionState::Cancelled,
                    None,
                    None,
                );
                return Ok(QuestionWaitResult::Aborted {
                    reason: "ASK_ABORTED".to_string(),
                });
            }

            elapsed_in_window_ms = elapsed_in_window_ms.saturating_add(budget.poll_interval_ms);
            total_elapsed_ms = total_elapsed_ms.saturating_add(budget.poll_interval_ms);

            // 检查当前窗口是否到期
            if elapsed_in_window_ms >= current_window_ms {
                if total_elapsed_ms >= budget.total_budget_ms {
                    // 总预算耗尽 -> 挂起
                    let handoff = actual_record
                        .handoff_text
                        .as_deref()
                        .unwrap_or(DEFAULT_SUSPENDED_HANDOFF)
                        .to_string();

                    let _ = self.store.cas_update_state(
                        &actual_record.question_id,
                        UserQuestionState::Pending,
                        UserQuestionState::Suspended,
                        None,
                        Some(&handoff),
                    );

                    self.sink.emit_question_event(
                        &actual_record.lane_id,
                        &part_id,
                        &actual_record.tool_call_id,
                        "suspended",
                        &questions,
                        &[],
                        Some(&handoff),
                        current_deadline_at_ms,
                    );

                    return Ok(QuestionWaitResult::Suspended {
                        handoff_text: handoff,
                    });
                }

                // 窗口到期且预算未耗尽 -> 退避展期 (extend)
                current_window_ms = current_window_ms
                    .saturating_mul(budget.backoff_factor as u64)
                    .min(budget.total_budget_ms.saturating_sub(total_elapsed_ms));
                elapsed_in_window_ms = 0;
                current_deadline_at_ms =
                    current_deadline_at_ms.saturating_add(current_window_ms as i64);

                let _ = self.store.cas_update_state(
                    &actual_record.question_id,
                    UserQuestionState::Pending,
                    UserQuestionState::Pending,
                    Some(current_deadline_at_ms),
                    None,
                );

                self.sink.emit_question_event(
                    &actual_record.lane_id,
                    &part_id,
                    &actual_record.tool_call_id,
                    "extended",
                    &questions,
                    &[],
                    actual_record.handoff_text.as_deref(),
                    current_deadline_at_ms,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_session::UserQuestionStoreError;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Default)]
    struct TestQuestionStore {
        insert_calls: AtomicUsize,
        update_state_calls: AtomicUsize,
    }

    impl UserQuestionStore for TestQuestionStore {
        fn insert_question(
            &self,
            record: &UserQuestionRecord,
        ) -> Result<UserQuestionRecord, UserQuestionStoreError> {
            self.insert_calls.fetch_add(1, Ordering::SeqCst);
            Ok(record.clone())
        }

        fn get_question_by_id(
            &self,
            _id: &str,
        ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError> {
            Ok(None)
        }

        fn get_question_by_call_id(
            &self,
            _call_id: &str,
        ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError> {
            Ok(None)
        }

        fn list_questions_by_lane(
            &self,
            _lane_id: &str,
            _state: Option<UserQuestionState>,
            _limit: usize,
        ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError> {
            Ok(Vec::new())
        }

        fn cas_settle_answer(
            &self,
            _call_id: &str,
            _answer_json: &str,
            _answered_at_ms: i64,
            _channel: Option<&str>,
            _delivery_mode: Option<&str>,
        ) -> Result<UserQuestionRecord, UserQuestionStoreError> {
            Err(UserQuestionStoreError::NotFound(
                "test_not_found".to_string(),
            ))
        }

        fn cas_update_state(
            &self,
            _question_id: &str,
            _from: UserQuestionState,
            _to: UserQuestionState,
            _deadline_at_ms: Option<i64>,
            _handoff_text: Option<&str>,
        ) -> Result<bool, UserQuestionStoreError> {
            self.update_state_calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }

        fn list_unreconciled_answered(
            &self,
            _limit: usize,
        ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError> {
            Ok(Vec::new())
        }

        fn mark_delivered(
            &self,
            _question_id: &str,
            _delivery_mode: &str,
        ) -> Result<bool, UserQuestionStoreError> {
            Ok(true)
        }

        fn mark_expired_questions(
            &self,
            _now_ms: i64,
            _limit: usize,
        ) -> Result<usize, UserQuestionStoreError> {
            Ok(0)
        }
    }

    fn test_record(call_id: &str) -> UserQuestionRecord {
        UserQuestionRecord {
            question_id: format!("uq-{call_id}"),
            idempotency_key: format!("idem-{call_id}"),
            lane_id: "lane-1".to_string(),
            session_id: "sess-1".to_string(),
            interaction_id: "inter-1".to_string(),
            turn_id: "turn-1".to_string(),
            tool_call_id: call_id.to_string(),
            questions_json: r#"[{"id":"q1","question":"确认？"}]"#.to_string(),
            handoff_text: Some("等待用户决策".to_string()),
            state: UserQuestionState::Pending,
            deadline_at_ms: 10_000,
            answer_json: None,
            answered_at_ms: None,
            answered_channel: None,
            delivered: false,
            delivery_mode: None,
            created_at_ms: 1_000,
            expires_at_ms: 86_400_000,
        }
    }

    #[test]
    fn test_waiter_raii_unregisters_on_normal_exit_and_panic() {
        let store = Arc::new(TestQuestionStore::default());
        let host = UserQuestionHost::new(store, Arc::new(NoopUserQuestionEventSink), None);

        let rec = test_record("call-panic");

        // 在新线程中运行并注入 panic
        let host_clone = host.clone();
        let handle = std::thread::spawn(move || {
            let (tx, _) = tokio::sync::oneshot::channel();
            host_clone
                .waiters
                .lock()
                .unwrap()
                .insert(rec.tool_call_id.clone(), tx);
            let _guard = WaiterGuard {
                call_id: rec.tool_call_id.clone(),
                waiters: host_clone.waiters.clone(),
            };
            panic!("test panic");
        });
        let _ = handle.join();

        // 即使发生 panic，waiter 必须被注销
        assert_eq!(host.waiter_count(), 0);
    }

    #[test]
    fn test_answer_within_budget_returns_immediately() {
        let store = Arc::new(TestQuestionStore::default());
        let host = UserQuestionHost::new(store.clone(), Arc::new(NoopUserQuestionEventSink), None);

        let rec = test_record("call-fast");
        let budget = QuestionWaitBudget {
            initial_window_ms: 5_000,
            backoff_factor: 2,
            total_budget_ms: 10_000,
            poll_interval_ms: 20,
        };

        let host_for_delivery = host.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            host_for_delivery.deliver_to_waiter(
                "call-fast",
                QuestionAnswerDelivery {
                    answer_json: r#"[{"id":"q1","selected":["继续"]}]"#.to_string(),
                    channel: Some("desktop".to_string()),
                },
            );
        });

        let cancel = RunCancelToken::default();
        let outcome = host
            .wait_for_answer(&rec, budget, &cancel)
            .expect("等待应正常返回");

        match outcome {
            QuestionWaitResult::Answered {
                answer_json,
                channel,
            } => {
                assert!(answer_json.contains("继续"));
                assert_eq!(channel.as_deref(), Some("desktop"));
            }
            other => panic!("期望 Answered，得到 {other:?}"),
        }

        // 等待期间无额外 store 更新调用（只在最开始 insert 一次）
        assert_eq!(store.insert_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.update_state_calls.load(Ordering::SeqCst), 0);
        assert_eq!(host.waiter_count(), 0);
    }

    #[test]
    fn test_budget_exhaustion_suspends() {
        let store = Arc::new(TestQuestionStore::default());
        let host = UserQuestionHost::new(store.clone(), Arc::new(NoopUserQuestionEventSink), None);

        let rec = test_record("call-timeout");
        // 设置极短的测试预算：首窗 40ms，总预算 80ms，轮询 20ms
        let budget = QuestionWaitBudget {
            initial_window_ms: 40,
            backoff_factor: 2,
            total_budget_ms: 80,
            poll_interval_ms: 20,
        };

        let cancel = RunCancelToken::default();
        let outcome = host
            .wait_for_answer(&rec, budget, &cancel)
            .expect("等待应正常返回");

        match outcome {
            QuestionWaitResult::Suspended { handoff_text } => {
                assert_eq!(handoff_text, "等待用户决策");
            }
            other => panic!("期望 Suspended，得到 {other:?}"),
        }

        // 窗口到期 extend 1 次 + 预算耗尽 suspend 1 次 = update_state 2 次
        assert_eq!(store.update_state_calls.load(Ordering::SeqCst), 2);
        assert_eq!(host.waiter_count(), 0);
    }

    #[test]
    fn test_cancel_token_aborts_promptly() {
        let store = Arc::new(TestQuestionStore::default());
        let host = UserQuestionHost::new(store.clone(), Arc::new(NoopUserQuestionEventSink), None);

        let rec = test_record("call-cancel");
        let budget = QuestionWaitBudget {
            initial_window_ms: 10_000,
            backoff_factor: 2,
            total_budget_ms: 20_000,
            poll_interval_ms: 20,
        };

        let cancel = RunCancelToken::default();
        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            cancel_clone.cancel();
        });

        let outcome = host
            .wait_for_answer(&rec, budget, &cancel)
            .expect("等待应正常返回");

        match outcome {
            QuestionWaitResult::Aborted { reason } => {
                assert_eq!(reason, "ASK_ABORTED");
            }
            other => panic!("期望 Aborted，得到 {other:?}"),
        }

        assert_eq!(host.waiter_count(), 0);
    }
}
