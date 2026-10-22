//! 单 Session Action Pump。

use crate::model::{ActionExecutionRequest, SessionExecutionContext};
use crate::ports::{EventPublisher, RuntimeClock, TurnExecutor};
use axon_domain::{
    DaemonSessionId, ExecutionId, GoalStatus, JarvisEvent, JarvisEventId, SessionActionId,
    SessionActionStatus, SessionRuntimeStatus,
};
use axon_session::{DaemonHostRecord, RuntimeStore, RuntimeStoreError, SessionActionRecord};
use thiserror::Error;

#[cfg(test)]
use crate::model::TurnExecutionOutcome;
#[cfg(test)]
use axon_domain::GoalId;

const MAX_EXECUTION_ATTEMPTS: u32 = 3;

/// Session Worker 失败。
#[derive(Debug, Error)]
pub enum ExecutionWorkerError {
    /// 持久化错误。
    #[error(transparent)]
    Store(#[from] RuntimeStoreError),
    /// Agent 执行错误；可重试错误进入持久退避，永久错误直接进入死信。
    #[error("Agent Action 执行失败：{0}")]
    Execution(String),
    /// 事件订阅者错误。
    #[error("Session 事件发布失败：{0}")]
    Publish(String),
    /// Work Ledger 门禁读取或拒绝。
    #[error("Execution Work Ledger 门禁拒绝：{0}")]
    Gate(String),
}

/// 单个 Session 的串行 Action Pump。
pub struct ExecutionWorker<S, E, P, C> {
    store: S,
    executor: E,
    publisher: P,
    clock: C,
    host: DaemonHostRecord,
    session_id: DaemonSessionId,
    action_lease_duration_ms: u64,
}

impl<S, E, P, C> ExecutionWorker<S, E, P, C>
where
    S: RuntimeStore,
    E: TurnExecutor,
    P: EventPublisher,
    C: RuntimeClock,
{
    /// 创建单 Session Worker。
    pub fn new(
        store: S,
        executor: E,
        publisher: P,
        clock: C,
        host: DaemonHostRecord,
        session_id: DaemonSessionId,
        action_lease_duration_ms: u64,
    ) -> Self {
        Self {
            store,
            executor,
            publisher,
            clock,
            host,
            session_id,
            action_lease_duration_ms,
        }
    }

    /// 领取并执行至多一个 Action；同一 Session 绝不并发执行 Provider turn。
    pub fn pump_once(&self) -> Result<Option<SessionActionId>, ExecutionWorkerError> {
        let now_ms = self.clock.now_ms();
        let Some(action) = self.claim_action(now_ms)? else {
            return Ok(None);
        };
        let runtime = self.store.get_session(&self.session_id)?;
        if self.cancel_stale_action(&action, runtime.context_version, now_ms)? {
            return Ok(Some(action.action_id));
        }
        if !self.passes_ledger_gate(&action, now_ms)? {
            return Ok(Some(action.action_id));
        }
        self.mark_running(&action, now_ms)?;
        let outcome = match self.executor.execute(ActionExecutionRequest {
            action: action.clone(),
            context: SessionExecutionContext {
                run_session_id: runtime.run_session_id,
                cwd: runtime.cwd,
                context_version: runtime.context_version,
            },
            runtime_profile: crate::JarvisRuntimeSelector::for_background_execution(),
        }) {
            Ok(outcome) => outcome,
            Err(error) => {
                self.settle_failure(&action, error.clone())?;
                return Err(ExecutionWorkerError::Execution(error.to_string()));
            }
        };
        let checkpoint = outcome.checkpoint.clone();
        if let Err(error) = self.commit_outcome(&action, outcome, &checkpoint) {
            if matches!(error, ExecutionWorkerError::Gate(_)) {
                let _ = self.cancel_for_gate(&action, self.clock.now_ms());
            }
            return Err(error);
        }
        Ok(Some(action.action_id))
    }

    fn claim_action(
        &self,
        now_ms: u64,
    ) -> Result<Option<SessionActionRecord>, ExecutionWorkerError> {
        let action = self.store.claim_next_action(
            &self.session_id,
            &self.host,
            now_ms,
            self.action_lease_duration_ms,
        )?;
        if action.is_none() {
            self.store.set_session_status(
                &self.session_id,
                &self.host,
                SessionRuntimeStatus::Idle,
                now_ms,
            )?;
        }
        Ok(action)
    }

    fn cancel_stale_action(
        &self,
        action: &SessionActionRecord,
        context_version: u64,
        now_ms: u64,
    ) -> Result<bool, ExecutionWorkerError> {
        if action.context_version == Some(context_version) {
            return Ok(false);
        }
        match self.store.update_action_status(
            &action.action_id,
            &self.session_id,
            &self.host,
            SessionActionStatus::Cancelled,
            now_ms,
        ) {
            Ok(()) | Err(RuntimeStoreError::Fenced) => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    fn mark_running(
        &self,
        action: &SessionActionRecord,
        now_ms: u64,
    ) -> Result<(), ExecutionWorkerError> {
        self.store.set_session_status(
            &self.session_id,
            &self.host,
            SessionRuntimeStatus::Running,
            now_ms,
        )?;
        let goal_id = match action.goal_id.clone() {
            Some(goal_id) => goal_id,
            None => {
                let execution_id = ExecutionId::new(action.action_id.as_str())
                    .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
                let Some(execution) = self
                    .store
                    .get_execution(&execution_id)
                    .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
                else {
                    return Ok(());
                };
                execution.goal_id
            }
        };
        let event_key = format!(
            "execution.started:{}:{}",
            action.action_id.as_str(),
            action.attempt
        );
        let event = JarvisEvent {
            event_id: JarvisEventId::new(format!(
                "event-started-{}-{}",
                action.action_id.as_str(),
                action.attempt
            ))
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?,
            sequence: 0,
            event_type: "execution.started".to_owned(),
            idempotency_key: event_key,
            cause_event_id: None,
            payload_json: serde_json::json!({
                "actionId": action.action_id.as_str(),
                "sessionId": self.session_id.as_str(),
                "startedAtMs": now_ms,
            })
            .to_string(),
            occurred_at_ms: now_ms,
        };
        if let Some(mut goal) = self
            .store
            .get_goal(&goal_id)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
            && goal.status == GoalStatus::Proposed
        {
            let revision = goal.revision;
            goal.transition(GoalStatus::Active, now_ms)
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
            self.store
                .update_goal_with_event(&goal, revision, &event)
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
        } else {
            self.store
                .append_ledger_event(&event)
                .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
        }
        Ok(())
    }

    fn passes_ledger_gate(
        &self,
        action: &SessionActionRecord,
        now_ms: u64,
    ) -> Result<bool, ExecutionWorkerError> {
        let goal_id = match action.goal_id.clone() {
            Some(goal_id) => goal_id,
            None => {
                let execution_id = ExecutionId::new(action.action_id.as_str())
                    .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
                let Some(execution) = self
                    .store
                    .get_execution(&execution_id)
                    .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
                else {
                    return Ok(true);
                };
                execution.goal_id
            }
        };
        let Some(goal) = self
            .store
            .get_goal(&goal_id)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
        else {
            if goal_id.as_str() == action.action_id.as_str() {
                return Ok(true);
            }
            self.cancel_for_gate(action, now_ms)?;
            return Ok(false);
        };
        if goal.status != GoalStatus::Active && goal.status != GoalStatus::Proposed {
            self.cancel_for_gate(action, now_ms)?;
            return Ok(false);
        }
        let allowed = self
            .store
            .get_latest_decision(&goal.scope, now_ms)
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?
            .is_some_and(|decision| decision.is_effective_at(now_ms));
        if !allowed {
            self.cancel_for_gate(action, now_ms)?;
        }
        Ok(allowed)
    }

    fn cancel_for_gate(
        &self,
        action: &SessionActionRecord,
        now_ms: u64,
    ) -> Result<(), ExecutionWorkerError> {
        if let Ok(execution_id) = ExecutionId::new(action.action_id.as_str())
            && let Ok(Some(execution)) = self.store.get_execution(&execution_id)
            && let Some(ref lease) = execution.lease
        {
            let _ = self
                .store
                .cancel_execution(&execution_id, &lease.owner, &lease.token, now_ms);
        }
        let event_key = format!("execution.cancelled:{}", action.action_id.as_str());
        let _ = self.store.append_ledger_event(&JarvisEvent {
            event_id: JarvisEventId::new(format!("event-cancelled-{}", action.action_id.as_str()))
                .unwrap_or_else(|_| JarvisEventId::new("event-cancelled").unwrap()),
            sequence: 0,
            event_type: "execution.cancelled".to_owned(),
            idempotency_key: event_key,
            cause_event_id: None,
            payload_json: serde_json::json!({
                "actionId": action.action_id.as_str(),
                "sessionId": self.session_id.as_str(),
                "cancelledAtMs": now_ms,
            })
            .to_string(),
            occurred_at_ms: now_ms,
        });
        self.store
            .update_action_status(
                &action.action_id,
                &self.session_id,
                &self.host,
                SessionActionStatus::Cancelled,
                now_ms,
            )
            .map_err(|error| ExecutionWorkerError::Gate(error.to_string()))?;
        self.store.set_session_status(
            &self.session_id,
            &self.host,
            SessionRuntimeStatus::Idle,
            now_ms,
        )?;
        Ok(())
    }
}

#[path = "execution_worker_outcome.rs"]
mod outcome;

#[path = "execution_worker_settlement.rs"]
mod settlement;

#[cfg(test)]
#[path = "execution_worker_completion_tests.rs"]
mod completion_tests;
#[cfg(test)]
#[path = "execution_worker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "execution_worker_goal_id_tests.rs"]
mod goal_id_tests;

#[cfg(test)]
#[path = "execution_worker_failure_tests.rs"]
mod failure_tests;
