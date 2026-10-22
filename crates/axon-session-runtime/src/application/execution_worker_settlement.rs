//! Execution Worker 的结构化失败结算。
//!
//! 失败分类由执行边界给出（`TurnFailureKind`），本模块只负责把分类落成持久事实：
//! - 可重试失败：`fail_action` 退避重排，并追加 `execution.waiting` 事件；
//! - 已形成结论的失败（确定性失败、外部结果未知、重试耗尽）：
//!   同一事务提交 Ledger、Session 收尾和 checkpoint，避免结论丢失。
//!
//! 顺序不可颠倒：Ledger 是用户可见的唯一事实来源，先释放 lease 再补 Ledger
//! 会让失败结论永久丢失。

use super::{ExecutionWorker, ExecutionWorkerError, MAX_EXECUTION_ATTEMPTS};
use crate::model::{TurnExecutionError, TurnFailureKind};
use crate::ports::{EventPublisher, RuntimeClock};
use axon_domain::{
    ExecutionId, ExecutionStatus, GoalId, GoalRecord, GoalStatus, JarvisEvent, JarvisEventId,
    ResultErrorKind, ResultId, ResultRecord,
};
use axon_session::{CheckpointTelemetry, ExecutionSettlement, RuntimeStore, SessionActionRecord};

const FAILED_LEDGER_EVENT: &str = "execution.failed";
const UNCERTAIN_LEDGER_EVENT: &str = "execution.uncertain";
const RETRY_LEDGER_EVENT: &str = "execution.waiting";
const FAILED_SESSION_EVENT: &str = "turn_failed";
const UNCERTAIN_SESSION_EVENT: &str = "turn_uncertain";
const FAILURE_RESPONSE_PREFIX: &str = "本次处理失败：";

/// 失败形成结论后的持久形态。
#[derive(Debug, Clone, Copy)]
struct FailureConclusion {
    execution_status: ExecutionStatus,
    goal_status: GoalStatus,
    error_kind: ResultErrorKind,
    ledger_event_type: &'static str,
    session_event_type: &'static str,
    result_id_prefix: &'static str,
}

const FAILED_CONCLUSION: FailureConclusion = FailureConclusion {
    execution_status: ExecutionStatus::Failed,
    goal_status: GoalStatus::Failed,
    error_kind: ResultErrorKind::Runtime,
    ledger_event_type: FAILED_LEDGER_EVENT,
    session_event_type: FAILED_SESSION_EVENT,
    result_id_prefix: "result-failed",
};

/// 外部结果未知：禁止自动重试，必须由后续 Decision 显式推进。
const UNCERTAIN_CONCLUSION: FailureConclusion = FailureConclusion {
    execution_status: ExecutionStatus::Uncertain,
    goal_status: GoalStatus::Waiting,
    error_kind: ResultErrorKind::Uncertain,
    ledger_event_type: UNCERTAIN_LEDGER_EVENT,
    session_event_type: UNCERTAIN_SESSION_EVENT,
    result_id_prefix: "result-uncertain",
};

/// 失败类别对应的重试与结论策略。
#[derive(Debug, Clone, Copy)]
struct FailurePolicy {
    max_attempts: u32,
    conclusion: Option<FailureConclusion>,
}

impl FailurePolicy {
    const fn for_kind(kind: TurnFailureKind) -> Self {
        match kind {
            TurnFailureKind::Retryable => Self {
                max_attempts: MAX_EXECUTION_ATTEMPTS,
                conclusion: None,
            },
            TurnFailureKind::Permanent => Self {
                max_attempts: 1,
                conclusion: Some(FAILED_CONCLUSION),
            },
            TurnFailureKind::ExternalOutcomeUnknown => Self {
                max_attempts: 1,
                conclusion: Some(UNCERTAIN_CONCLUSION),
            },
        }
    }

    /// 判断本次失败是否必须立即形成结论。
    fn conclusion_for(self, attempt: u32) -> Option<FailureConclusion> {
        match self.conclusion {
            Some(conclusion) => Some(conclusion),
            None if attempt >= self.max_attempts => Some(FAILED_CONCLUSION),
            None => None,
        }
    }
}

impl<S, E, P, C> ExecutionWorker<S, E, P, C>
where
    S: RuntimeStore,
    E: crate::ports::TurnExecutor,
    P: EventPublisher,
    C: RuntimeClock,
{
    /// 结算一次 Turn 执行失败。
    pub(super) fn settle_failure(
        &self,
        action: &SessionActionRecord,
        failure: TurnExecutionError,
    ) -> Result<(), ExecutionWorkerError> {
        let now_ms = self.clock.now_ms();
        let kind = failure.kind();
        let policy = FailurePolicy::for_kind(kind);
        let message = failure.into_message();
        let transport_context = serde_json::from_str::<serde_json::Value>(&action.payload_json)
            .ok()
            .and_then(|payload| payload.get("transportContext").cloned());
        let payload = serde_json::json!({
            "actionId": action.action_id,
            "kind": action.kind,
            "attempt": action.attempt,
            "classification": kind.as_str(),
            "error": message,
            "response": format!("{FAILURE_RESPONSE_PREFIX}{message}"),
            "transportContext": transport_context,
        })
        .to_string();
        if let Some(conclusion) = policy.conclusion_for(action.attempt) {
            return self.commit_conclusion(action, conclusion, message, &payload, now_ms);
        }
        let records = self.store.fail_action(
            &action.action_id,
            &self.session_id,
            &self.host,
            &payload,
            policy.max_attempts,
            now_ms,
        )?;
        for record in records {
            self.publisher
                .publish(&record, &record.cursor())
                .map_err(ExecutionWorkerError::Publish)?;
        }
        self.append_retry_event(action, &message, now_ms);
        Ok(())
    }

    /// 退避重排：事件重复写入由幂等键消化，不影响重试事实。
    fn append_retry_event(&self, action: &SessionActionRecord, error: &str, now_ms: u64) {
        let event = JarvisEvent {
            event_id: JarvisEventId::new(format!(
                "event-waiting-{}-{}",
                action.action_id.as_str(),
                action.attempt
            ))
            .unwrap_or_else(|_| JarvisEventId::new("event-waiting").unwrap()),
            sequence: 0,
            event_type: RETRY_LEDGER_EVENT.to_owned(),
            idempotency_key: format!(
                "execution.waiting:{}:{}",
                action.action_id.as_str(),
                action.attempt
            ),
            cause_event_id: None,
            payload_json: serde_json::json!({
                "actionId": action.action_id.as_str(),
                "attempt": action.attempt,
                "reason": "retry_backoff",
                "error": error,
            })
            .to_string(),
            occurred_at_ms: now_ms,
        };
        let _ = self.store.append_ledger_event(&event);
    }

    /// 同一事务提交失败结论与 Session 收尾。
    fn commit_conclusion(
        &self,
        action: &SessionActionRecord,
        conclusion: FailureConclusion,
        message: String,
        payload: &str,
        now_ms: u64,
    ) -> Result<(), ExecutionWorkerError> {
        let execution_id = ExecutionId::new(action.action_id.as_str())
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
        let goal = self.load_conclusive_goal(action, &execution_id)?;
        let Some(goal) = goal else {
            // 既有事实缺失或已取消时不覆盖 Ledger，只补齐并发布 Session 事件。
            let record = self.store.append_event(
                &self.session_id,
                &self.host,
                conclusion.session_event_type,
                payload,
                now_ms,
            )?;
            self.publisher
                .publish(&record, &record.cursor())
                .map_err(ExecutionWorkerError::Publish)?;
            return Ok(());
        };
        let settlement = if let Some(execution) = self
            .store
            .get_execution(&execution_id)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
            && let Some(lease) = execution.lease.clone()
        {
            let (result, event) = conclusion_records(action, &goal, &conclusion, message, now_ms);
            let mut terminal_execution = execution;
            terminal_execution.status = conclusion.execution_status;
            terminal_execution.updated_at_ms = now_ms;
            let mut updated_goal = goal.clone();
            updated_goal.status = conclusion.goal_status;
            updated_goal.revision = goal.revision.saturating_add(1);
            updated_goal.updated_at_ms = now_ms;
            if conclusion.goal_status == GoalStatus::Waiting {
                updated_goal.waiting_reason =
                    Some("外部副作用结果未知，等待受控恢复证据".to_owned());
            }
            Some(ExecutionSettlement {
                execution: terminal_execution,
                owner: lease.owner,
                lease_token: lease.token,
                result,
                goal: updated_goal,
                expected_goal_revision: goal.revision,
                event,
            })
        } else {
            return Err(ExecutionWorkerError::Gate(
                "Execution 缺少结算 lease".to_owned(),
            ));
        };
        let records = self.store.commit_action(
            &action.action_id,
            &self.session_id,
            &self.host,
            &[(conclusion.session_event_type.to_owned(), payload.to_owned())],
            None,
            settlement.as_ref(),
            &CheckpointTelemetry::default(),
            now_ms,
        )?;
        for record in records {
            self.publisher
                .publish(&record, &record.cursor())
                .map_err(ExecutionWorkerError::Publish)?;
        }
        Ok(())
    }

    /// 读取结论所需的 Goal；已取消时返回 `None`，避免覆写取消事实。
    fn load_conclusive_goal(
        &self,
        action: &SessionActionRecord,
        execution_id: &ExecutionId,
    ) -> Result<Option<GoalRecord>, ExecutionWorkerError> {
        let goal_id = match action.goal_id.clone() {
            Some(goal_id) => goal_id,
            None => match self
                .store
                .get_execution(execution_id)
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
            {
                Some(execution) => execution.goal_id,
                None => GoalId::new(action.action_id.as_str())
                    .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?,
            },
        };
        let goal = self
            .store
            .get_goal(&goal_id)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
        Ok(goal.filter(|goal| goal.status != GoalStatus::Cancelled))
    }
}

fn conclusion_records(
    action: &SessionActionRecord,
    goal: &GoalRecord,
    conclusion: &FailureConclusion,
    message: String,
    now_ms: u64,
) -> (ResultRecord, JarvisEvent) {
    let execution_id = ExecutionId::new(action.action_id.as_str())
        .unwrap_or_else(|_| ExecutionId::new("execution-unknown").unwrap());
    let result = ResultRecord::failure(
        ResultId::new(format!(
            "{}-{}",
            conclusion.result_id_prefix,
            action.action_id.as_str()
        ))
        .unwrap_or_else(|_| ResultId::new(conclusion.result_id_prefix).unwrap()),
        goal.goal_id.clone(),
        execution_id,
        message,
        conclusion.error_kind,
        now_ms,
    );
    let event = JarvisEvent {
        event_id: JarvisEventId::new(format!(
            "event-{}-{}",
            conclusion.ledger_event_type,
            action.action_id.as_str()
        ))
        .unwrap_or_else(|_| JarvisEventId::new("event-execution-conclusion").unwrap()),
        sequence: 0,
        event_type: conclusion.ledger_event_type.to_owned(),
        idempotency_key: format!(
            "{}:{}",
            conclusion.ledger_event_type,
            action.action_id.as_str()
        ),
        cause_event_id: None,
        payload_json: serde_json::to_string(&result).unwrap_or_default(),
        occurred_at_ms: now_ms,
    };
    (result, event)
}
