//! 用户问答统一答案入口应用服务与对账修复（UQ-005, UQ-006）。

use std::collections::HashSet;
use std::sync::Arc;

use axon_agent_runtime::MAX_RECONCILE_BATCH;
use axon_domain::{UserAnswerItem, UserQuestionItem, UserQuestionRecord, UserQuestionState};
use axon_session::UserQuestionStore;
use thiserror::Error;

use super::content::encode_question_answer_message;
use super::host::{QuestionAnswerDelivery, UserQuestionEventSink, UserQuestionHost};

/// 投递方式：答案直接回灌给正在等待的工具调用。
const DELIVERY_MODE_TOOL_RETURN: &str = "tool_return";
/// 投递方式：答案作为一条用户消息投进车道。
const DELIVERY_MODE_USER_MESSAGE: &str = "user_message";

/// 答案提交统一业务错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AnswerError {
    /// 答案格式不合法或缺失必填项。
    #[error("BAD_ANSWER: {0}")]
    BadAnswer(String),
    /// 未知的问题调用标识。
    #[error("UNKNOWN_CALL: {0}")]
    UnknownCall(String),
    /// 问题已处于终态或已被其他渠道结算。
    #[error("ALREADY_SETTLED: {0}")]
    AlreadySettled(String),
    /// 车道不匹配。
    #[error("LANE_MISMATCH: {0}")]
    LaneMismatch(String),
    /// 内部存储或处理错误。
    #[error("INTERNAL_ERROR: {0}")]
    Internal(String),
}

/// 答案持久化为用户消息的注入接口。
pub trait QuestionMessageSteerer: Send + Sync {
    /// 向指定车道注入一条带稳定幂等键的用户答案消息。
    fn steer_answer_message(
        &self,
        lane_id: &str,
        session_id: &str,
        idempotency_key: &str,
        content: &str,
    ) -> Result<(), String>;
}

/// 空消息注入器（用于测试或无消息通道环境）。
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopQuestionMessageSteerer;

impl QuestionMessageSteerer for NoopQuestionMessageSteerer {
    fn steer_answer_message(
        &self,
        _lane_id: &str,
        _session_id: &str,
        _idempotency_key: &str,
        _content: &str,
    ) -> Result<(), String> {
        Ok(())
    }
}

/// 用户问答应用服务。
#[derive(Clone)]
pub struct UserQuestionAnswerService {
    store: Arc<dyn UserQuestionStore>,
    host: Option<UserQuestionHost>,
    sink: Arc<dyn UserQuestionEventSink>,
    steerer: Arc<dyn QuestionMessageSteerer>,
}

impl UserQuestionAnswerService {
    /// 创建答案服务实例。
    pub fn new(
        store: Arc<dyn UserQuestionStore>,
        host: Option<UserQuestionHost>,
        sink: Arc<dyn UserQuestionEventSink>,
        steerer: Arc<dyn QuestionMessageSteerer>,
    ) -> Self {
        Self {
            store,
            host,
            sink,
            steerer,
        }
    }

    /// 统一答案提交流程：
    /// 1. 校验（call_id 存在、车道匹配、答案结构匹配、状态为 pending/suspended）；
    /// 2. CAS 结算权威记录；
    /// 3. 二选一互斥投递（优先活等待者，否则持久用户消息）；
    /// 4. 发射 settled 事件；
    /// 5. 触发一轮限量对账，兜住历史崩溃窗口留下的未投递记录。
    pub fn submit_answer(
        &self,
        lane_id: &str,
        call_id: &str,
        answers: Vec<UserAnswerItem>,
        channel: &str,
    ) -> Result<UserQuestionRecord, AnswerError> {
        // 1. 查询既有记录
        let record = self
            .store
            .get_question_by_call_id(call_id)
            .map_err(|e| AnswerError::Internal(e.to_string()))?
            .or_else(|| self.store.get_question_by_id(call_id).ok().flatten())
            .ok_or_else(|| AnswerError::UnknownCall(call_id.to_string()))?;

        // 校验车道一致性
        if !lane_id.is_empty() && record.lane_id != lane_id {
            return Err(AnswerError::LaneMismatch(format!(
                "问题属于车道 {}，收到作答车道 {}",
                record.lane_id, lane_id
            )));
        }

        // 校验状态是否可作答
        if record.state != UserQuestionState::Pending
            && record.state != UserQuestionState::Suspended
        {
            return Err(AnswerError::AlreadySettled(format!(
                "问题 {} 状态为 {}，无法作答",
                call_id,
                record.state.as_str()
            )));
        }

        // 校验答案集合
        let questions: Vec<UserQuestionItem> = serde_json::from_str(&record.questions_json)
            .map_err(|e| AnswerError::Internal(format!("问题解析失败: {e}")))?;

        validate_answers(&questions, &answers)?;

        let answers_json = serde_json::to_string(&answers)
            .map_err(|e| AnswerError::Internal(format!("答案序列化失败: {e}")))?;

        let now_ms = crate::tool_orchestrator::now_ms() as i64;

        // 2. CAS 结算
        let mut settled = self
            .store
            .cas_settle_answer(
                call_id,
                &answers_json,
                now_ms,
                Some(channel),
                // 投递方式取决于随后二选一的投递结果，在送达标记里一并落库。
                None,
            )
            .map_err(|e| match e {
                axon_session::UserQuestionStoreError::AlreadySettled(msg) => {
                    AnswerError::AlreadySettled(msg)
                }
                axon_session::UserQuestionStoreError::NotFound(msg) => {
                    AnswerError::UnknownCall(msg)
                }
                other => AnswerError::Internal(other.to_string()),
            })?;

        // 3. 二选一互斥投递
        let delivered_to_waiter = self.host.as_ref().is_some_and(|h| {
            h.deliver_to_waiter(
                &record.tool_call_id,
                QuestionAnswerDelivery {
                    answer_json: answers_json.clone(),
                    channel: Some(channel.to_string()),
                },
            )
        });

        if delivered_to_waiter {
            // 活等待者接收成功 -> tool_return 模式
            settled.delivered = true;
            settled.delivery_mode = Some(DELIVERY_MODE_TOOL_RETURN.to_string());
        } else {
            // 活等待者不存在 -> user_message 模式，投递到消息通道
            let msg_content = encode_question_answer_message(call_id, &questions, &answers);
            let idem_key = format!("question-answer-{}", settled.question_id);

            self.steerer
                .steer_answer_message(
                    &settled.lane_id,
                    &settled.session_id,
                    &idem_key,
                    &msg_content,
                )
                .map_err(|e| AnswerError::Internal(format!("消息投递失败: {e}")))?;

            settled.delivered = true;
            settled.delivery_mode = Some(DELIVERY_MODE_USER_MESSAGE.to_string());
        }

        let delivery_mode = settled
            .delivery_mode
            .clone()
            .unwrap_or_else(|| DELIVERY_MODE_USER_MESSAGE.to_string());
        self.store
            .mark_delivered(&settled.question_id, &delivery_mode)
            .map_err(|e| AnswerError::Internal(format!("投递标记落库失败: {e}")))?;

        // 4. 发射 settled 事件
        let part_id = crate::run_commands::question_part_id(&record.tool_call_id);
        self.sink.emit_question_event(
            &record.lane_id,
            &part_id,
            &record.tool_call_id,
            "settled",
            &questions,
            &answers,
            record.handoff_text.as_deref(),
            record.deadline_at_ms,
        );

        // 5. 对账：本次投递之后仍可能有更早的崩溃窗口留痕，按有界批次补投。
        self.reconcile_after_route();

        Ok(settled)
    }

    /// 答案路由结束后触发一轮限量对账；失败只留痕，不影响本次作答结果。
    fn reconcile_after_route(&self) {
        match self.reconcile_undelivered(MAX_RECONCILE_BATCH) {
            Ok(0) => {}
            Ok(count) => axon_logger::info(&format!("用户问答对账补投 {count} 条")),
            Err(error) => {
                axon_logger::warning(&format!("用户问答对账失败，等待下一轮重试: {error}"))
            }
        }
    }

    /// 对账补投：扫描 state='answered' AND delivered=0 的记录，以稳定 ID 补投并标记 delivered=1。
    pub fn reconcile_undelivered(&self, limit: usize) -> Result<usize, String> {
        let unreconciled = self
            .store
            .list_unreconciled_answered(limit)
            .map_err(|e| format!("对账扫描失败: {e}"))?;

        let mut reconciled_count = 0;
        for record in unreconciled {
            let questions: Vec<UserQuestionItem> =
                serde_json::from_str(&record.questions_json).unwrap_or_default();
            let answers: Vec<UserAnswerItem> = record
                .answer_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or_default();

            let msg_content =
                encode_question_answer_message(&record.tool_call_id, &questions, &answers);
            let idem_key = format!("question-answer-{}", record.question_id);

            self.steerer.steer_answer_message(
                &record.lane_id,
                &record.session_id,
                &idem_key,
                &msg_content,
            )?;

            // 补投同样要写投递方式：与首次投递共用同一条送达事实。
            self.store
                .mark_delivered(&record.question_id, DELIVERY_MODE_USER_MESSAGE)
                .map_err(|e| format!("标记投递状态失败: {e}"))?;
            reconciled_count += 1;
        }

        Ok(reconciled_count)
    }
}

fn validate_answers(
    questions: &[UserQuestionItem],
    answers: &[UserAnswerItem],
) -> Result<(), AnswerError> {
    if answers.is_empty() {
        return Err(AnswerError::BadAnswer("答案列表不能为空".to_string()));
    }

    let mut seen_ids = HashSet::new();
    for ans in answers {
        if !seen_ids.insert(&ans.id) {
            return Err(AnswerError::BadAnswer(format!(
                "重复的答案项 ID: {}",
                ans.id
            )));
        }

        let q = questions
            .iter()
            .find(|item| item.id == ans.id)
            .ok_or_else(|| AnswerError::BadAnswer(format!("答案对应的问题 ID 未知: {}", ans.id)))?;

        // 单选题校验：多选禁止
        if !q.multi_select && ans.selected.len() > 1 {
            return Err(AnswerError::BadAnswer(format!(
                "问题 {} 为单选题，不能提供多个选项",
                q.id
            )));
        }
    }

    // 缺项校验：每个问题都必须有作答，否则调用方会把「没答」当成默认选择。
    if let Some(missing) = questions.iter().find(|q| !seen_ids.contains(&q.id)) {
        return Err(AnswerError::BadAnswer(format!(
            "问题 {} 缺少作答",
            missing.id
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_session::UserQuestionStoreError;
    use std::sync::Mutex;

    #[derive(Default)]
    struct InMemoryQuestionStore {
        record: Mutex<Option<UserQuestionRecord>>,
        settle_calls: Mutex<usize>,
    }

    impl UserQuestionStore for InMemoryQuestionStore {
        fn insert_question(
            &self,
            record: &UserQuestionRecord,
        ) -> Result<UserQuestionRecord, UserQuestionStoreError> {
            *self.record.lock().unwrap() = Some(record.clone());
            Ok(record.clone())
        }

        fn get_question_by_id(
            &self,
            _id: &str,
        ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError> {
            Ok(self.record.lock().unwrap().clone())
        }

        fn get_question_by_call_id(
            &self,
            _call_id: &str,
        ) -> Result<Option<UserQuestionRecord>, UserQuestionStoreError> {
            Ok(self.record.lock().unwrap().clone())
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
            answer_json: &str,
            answered_at_ms: i64,
            channel: Option<&str>,
            delivery_mode: Option<&str>,
        ) -> Result<UserQuestionRecord, UserQuestionStoreError> {
            let mut lock = self.record.lock().unwrap();
            let mut calls = self.settle_calls.lock().unwrap();
            *calls += 1;

            if let Some(mut rec) = lock.clone() {
                if rec.state == UserQuestionState::Answered {
                    return Err(UserQuestionStoreError::AlreadySettled(
                        "already settled".into(),
                    ));
                }
                rec.state = UserQuestionState::Answered;
                rec.answer_json = Some(answer_json.to_string());
                rec.answered_at_ms = Some(answered_at_ms);
                rec.answered_channel = channel.map(str::to_string);
                rec.delivery_mode = delivery_mode.map(str::to_string);
                *lock = Some(rec.clone());
                Ok(rec)
            } else {
                Err(UserQuestionStoreError::NotFound("none".into()))
            }
        }

        fn cas_update_state(
            &self,
            _question_id: &str,
            _from: UserQuestionState,
            _to: UserQuestionState,
            _deadline_at_ms: Option<i64>,
            _handoff_text: Option<&str>,
        ) -> Result<bool, UserQuestionStoreError> {
            Ok(true)
        }

        fn list_unreconciled_answered(
            &self,
            _limit: usize,
        ) -> Result<Vec<UserQuestionRecord>, UserQuestionStoreError> {
            let lock = self.record.lock().unwrap();
            if let Some(rec) = lock.as_ref()
                && rec.state == UserQuestionState::Answered
                && !rec.delivered
            {
                return Ok(vec![rec.clone()]);
            }
            Ok(Vec::new())
        }

        fn mark_delivered(
            &self,
            _question_id: &str,
            delivery_mode: &str,
        ) -> Result<bool, UserQuestionStoreError> {
            let mut lock = self.record.lock().unwrap();
            if let Some(rec) = lock.as_mut() {
                rec.delivered = true;
                rec.delivery_mode = Some(delivery_mode.to_string());
            }
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

    #[derive(Default)]
    struct TestSteerer {
        steered_keys: Mutex<Vec<String>>,
    }

    impl QuestionMessageSteerer for TestSteerer {
        fn steer_answer_message(
            &self,
            _lane_id: &str,
            _session_id: &str,
            idempotency_key: &str,
            _content: &str,
        ) -> Result<(), String> {
            self.steered_keys
                .lock()
                .unwrap()
                .push(idempotency_key.to_string());
            Ok(())
        }
    }

    fn sample_record() -> UserQuestionRecord {
        UserQuestionRecord {
            question_id: "uq-100".to_string(),
            idempotency_key: "idem-100".to_string(),
            lane_id: "lane-main".to_string(),
            session_id: "sess-1".to_string(),
            interaction_id: "inter-1".to_string(),
            turn_id: "turn-1".to_string(),
            tool_call_id: "call-100".to_string(),
            questions_json: r#"[{"id":"q1","question":"确认部署？","options":[{"label":"是"},{"label":"否"}],"multi_select":false}]"#.to_string(),
            handoff_text: None,
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
    fn test_submit_answer_full_success_and_steer() {
        let store = Arc::new(InMemoryQuestionStore::default());
        store.insert_question(&sample_record()).unwrap();

        let steerer = Arc::new(TestSteerer::default());
        let service = UserQuestionAnswerService::new(
            store.clone(),
            None, // 无活等待者，必须退避至 steer 消息
            Arc::new(super::super::host::NoopUserQuestionEventSink),
            steerer.clone(),
        );

        let answers = vec![UserAnswerItem {
            id: "q1".to_string(),
            selected: vec!["是".to_string()],
            custom: None,
        }];

        let settled = service
            .submit_answer("lane-main", "call-100", answers, "desktop")
            .expect("提交答案应成功");

        assert_eq!(settled.state, UserQuestionState::Answered);
        assert_eq!(settled.delivery_mode.as_deref(), Some("user_message"));

        // 验证消息成功注入且带稳定 ID
        let steered = steerer.steered_keys.lock().unwrap().clone();
        assert_eq!(steered, vec!["question-answer-uq-100"]);
    }

    #[test]
    fn test_submit_answer_rejects_bad_answer_and_lane_mismatch() {
        let store = Arc::new(InMemoryQuestionStore::default());
        store.insert_question(&sample_record()).unwrap();

        let service = UserQuestionAnswerService::new(
            store,
            None,
            Arc::new(super::super::host::NoopUserQuestionEventSink),
            Arc::new(NoopQuestionMessageSteerer),
        );

        // 跨车道被拒
        let err = service
            .submit_answer(
                "lane-wrong",
                "call-100",
                vec![UserAnswerItem {
                    id: "q1".to_string(),
                    selected: vec!["是".to_string()],
                    custom: None,
                }],
                "desktop",
            )
            .expect_err("跨车道必须被拒");
        match err {
            AnswerError::LaneMismatch(_) => {}
            other => panic!("期望 LaneMismatch，得到 {other:?}"),
        }

        // 单选题选多个选项 -> BAD_ANSWER
        let err_multi = service
            .submit_answer(
                "lane-main",
                "call-100",
                vec![UserAnswerItem {
                    id: "q1".to_string(),
                    selected: vec!["是".to_string(), "否".to_string()],
                    custom: None,
                }],
                "desktop",
            )
            .expect_err("单选题多选必须被拒");
        match err_multi {
            AnswerError::BadAnswer(_) => {}
            other => panic!("期望 BadAnswer，得到 {other:?}"),
        }
    }

    #[test]
    fn test_reconciliation_only_once() {
        let store = Arc::new(InMemoryQuestionStore::default());
        let mut rec = sample_record();
        rec.state = UserQuestionState::Answered;
        rec.answer_json = Some(r#"[{"id":"q1","selected":["是"]}]"#.to_string());
        rec.delivered = false;
        store.insert_question(&rec).unwrap();

        let steerer = Arc::new(TestSteerer::default());
        let service = UserQuestionAnswerService::new(
            store,
            None,
            Arc::new(super::super::host::NoopUserQuestionEventSink),
            steerer.clone(),
        );

        // 首次对账成功补投 1 条
        let count = service.reconcile_undelivered(10).expect("对账");
        assert_eq!(count, 1);
        assert_eq!(
            steerer.steered_keys.lock().unwrap().as_slice(),
            &["question-answer-uq-100"]
        );

        // 重复对账补投数量为 0
        let count_again = service.reconcile_undelivered(10).expect("对账");
        assert_eq!(count_again, 0);
    }
}
