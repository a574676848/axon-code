//! 单 Session Action Pump。

use crate::model::{ActionExecutionRequest, SessionExecutionContext};
use crate::ports::{EventPublisher, RuntimeClock, TurnExecutor};
use axon_domain::{
    DaemonSessionId, ExecutionId, GoalStatus, JarvisEvent, JarvisEventId, SessionActionId,
    SessionActionStatus, SessionRuntimeStatus,
};
use axon_session::{DaemonHostRecord, RuntimeStore, RuntimeStoreError, SessionActionRecord};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use thiserror::Error;

#[cfg(test)]
use crate::model::TurnExecutionOutcome;
#[cfg(test)]
use axon_domain::GoalId;

const MAX_EXECUTION_ATTEMPTS: u32 = 3;
/// 续约周期与租约时长的比值：一半，因此一次失败仍有半程余量再试下一次。
const LEASE_RENEWAL_INTERVAL_DIVISOR: u64 = 2;
/// 停机时等待续约线程收口的时长上限。
///
/// 线程只在 `wait_timeout` 上睡眠，被唤醒后立即退出，正常路径收口在 1ms 量级；
/// 上限只用于兜住「线程恰好卡在一次 SQLite 写锁等待里」的极端情况：超过即脱手，
/// 线程持有自己的克隆，会在当前等待或写锁返回后自行退出，不拖住一轮的收尾。
const LEASE_RENEWAL_JOIN_TIMEOUT: Duration = Duration::from_millis(500);
/// 收口等待的轮询间隔。
const LEASE_RENEWAL_JOIN_POLL: Duration = Duration::from_millis(1);

/// 执行期的 Action 租约续约。
///
/// 设计要点（全部按「不拖慢一轮执行」取舍）：
/// - **线程只在执行期存在**：`start` 在 turn 开始后调用，守卫在 turn 收口时 Drop，
///   空闲期没有线程也没有写入；
/// - **零轮询**：线程阻塞在 `Condvar::wait_timeout` 上，每个周期只醒一次，
///   停机由 `notify_all` 立即放行，不靠短间隔空转；
/// - **每个周期一次按主键的 UPDATE**：命中 `jarvis_executions` 主键，代价与目标数量无关；
/// - **失败只记一次**：租约被抢（`Fenced`）时直接停表，其余错误记一行后继续等下一周期。
struct LeaseRenewal {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl LeaseRenewal {
    /// 在后台按租约的一半周期续租；租约时长为 0 时不启动。
    fn start<S, C>(
        store: S,
        action_id: SessionActionId,
        session_id: DaemonSessionId,
        host: DaemonHostRecord,
        clock: C,
        lease_duration_ms: u64,
    ) -> Option<Self>
    where
        S: RuntimeStore + Send + 'static,
        C: RuntimeClock + Send + 'static,
    {
        if lease_duration_ms == 0 {
            return None;
        }
        let interval =
            Duration::from_millis((lease_duration_ms / LEASE_RENEWAL_INTERVAL_DIVISOR).max(1));
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let thread_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("action-lease-renewal".to_owned())
            .spawn(move || {
                let mut reported = false;
                loop {
                    let (lock, cvar) = &*thread_stop;
                    let guard = lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let (guard, _) = cvar
                        .wait_timeout(guard, interval)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let stopping = *guard;
                    drop(guard);
                    if stopping {
                        break;
                    }
                    match store.renew_action_lease(
                        &action_id,
                        &session_id,
                        &host,
                        clock.now_ms(),
                        lease_duration_ms,
                    ) {
                        Ok(()) => {}
                        // 租约已不属于本次 claim：再续也不会生效，停表即可。
                        Err(RuntimeStoreError::Fenced) => break,
                        Err(error) => {
                            if !reported {
                                reported = true;
                                eprintln!(
                                    "Action 租约续约失败，按原周期重试: action={} error={error}",
                                    action_id.as_str()
                                );
                            }
                        }
                    }
                }
            })
            .ok()?;
        Some(Self {
            stop,
            handle: Some(handle),
        })
    }
}

impl Drop for LeaseRenewal {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        let (lock, cvar) = &*self.stop;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        cvar.notify_all();
        // 收口超时不阻塞调用方：线程只在等待上睡眠，唤醒后不会再持有连接。
        let deadline = std::time::Instant::now() + LEASE_RENEWAL_JOIN_TIMEOUT;
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            thread::sleep(LEASE_RENEWAL_JOIN_POLL);
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
    }
}

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
    S: RuntimeStore + Clone + Send + 'static,
    E: TurnExecutor,
    P: EventPublisher,
    C: RuntimeClock + Clone + Send + 'static,
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
        // 一次 provider 轮次可能长于租约：不续租时 supervisor 会把仍在跑的 Execution
        // 判成 uncertain 并中断，产出一段没有逐字稿的空会话。续约只在执行期存在，
        // 轮次一收口立即停表，因此空闲期零线程、零写入。
        let _renewal = LeaseRenewal::start(
            self.store.clone(),
            action.action_id.clone(),
            self.session_id.clone(),
            self.host.clone(),
            self.clock.clone(),
            self.action_lease_duration_ms,
        );
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

#[cfg(test)]
#[path = "execution_worker_lease_tests.rs"]
mod lease_tests;
