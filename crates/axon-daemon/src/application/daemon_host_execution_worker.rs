//! Daemon Host 的单 Session Worker 生命周期。

use super::daemon_host_execution_runtime::map_execution_worker_error;
use super::{
    DaemonHost, DaemonHostError, EXECUTION_WORKER_MAX_CONCURRENCY, EXECUTION_WORKER_QUEUE_CAPACITY,
};
use crate::application::egress_bridge::CommittedEventPublisher;
use crate::application::worker_supervisor::ExecutionWorkerLease;
use crate::ports::ProcessIdentity;
#[cfg(test)]
use axon_domain::ConsumerCursor;
use axon_domain::{DaemonSessionId, ExecutionId, SessionActionId};
use axon_session::RuntimeStore;
#[cfg(test)]
use axon_session::SessionEventRecord;
use axon_session_runtime::{EventPublisher, ExecutionWorker, RuntimeClock, TurnExecutor};
use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};
const WORKER_PANIC_MESSAGE: &str = "Session Worker 后台线程发生 panic，已进入执行权接管边界";

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 事件驱动唤醒一个 Session Worker；返回是否新建了 Worker。
    pub fn wake_session(&self, session_id: DaemonSessionId) -> bool {
        let Some(registration) = self.worker_registry.register(session_id.clone()) else {
            return false;
        };
        let pool = match self.execution_worker_pool.get_or_create_with_capacity(
            EXECUTION_WORKER_MAX_CONCURRENCY,
            EXECUTION_WORKER_QUEUE_CAPACITY,
        ) {
            Ok(pool) => pool,
            Err(_) => return false,
        };
        let worker_host = self.clone();
        if pool
            .try_execute(move || worker_host.run_execution_worker(session_id, registration))
            .is_err()
        {
            return false;
        }
        true
    }

    /// 执行某个 Session 的一个 Action。
    pub fn pump_session_once(
        &self,
        session_id: DaemonSessionId,
    ) -> Result<Option<SessionActionId>, DaemonHostError> {
        let host = self.started_host()?.clone();
        let action_id = ExecutionWorker::new(
            self.store.clone(),
            self.executor.clone(),
            CommittedEventPublisher::new(self.publisher.clone(), Arc::clone(&self.stream_bridge)),
            self.clock.clone(),
            host,
            session_id,
            self.action_lease_duration_ms,
        )
        .pump_once()
        .map_err(map_execution_worker_error)?;

        if let Some(ref id) = action_id {
            self.dispatch_result_delivery(id);
        }

        Ok(action_id)
    }

    /// 后台 Execution 形成结论后，触发完成通知的持久记录与派发。
    fn dispatch_result_delivery(&self, action_id: &SessionActionId) {
        let Ok(execution_id) = ExecutionId::new(action_id.as_str()) else {
            return;
        };
        if let Err(error) = self.record_execution_completion(&execution_id) {
            eprintln!(
                "完成通知记录失败，Result 保持未读等待补偿: execution={} error={error}",
                execution_id.as_str()
            );
        }
    }

    fn run_execution_worker(
        &self,
        session_id: DaemonSessionId,
        mut registration: ExecutionWorkerLease,
    ) {
        loop {
            let execution = catch_unwind(AssertUnwindSafe(|| {
                self.pump_session_once(session_id.clone())
            }));
            let reached_boundary = match execution {
                Ok(Ok(Some(_))) => false,
                Ok(Ok(None)) => true,
                Ok(Err(error)) => {
                    if !error.is_fenced() {
                        eprintln!("Session Worker 后台执行失败: {error}");
                    }
                    true
                }
                Err(_) => {
                    eprintln!("{WORKER_PANIC_MESSAGE}");
                    true
                }
            };
            if reached_boundary && !registration.continue_if_pending() {
                break;
            }
        }
    }
}

#[cfg(test)]
#[path = "daemon_host_execution_worker_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "daemon_host_channel_completion_tests.rs"]
mod channel_completion_tests;
#[cfg(test)]
#[path = "daemon_host_execution_delivery_tests.rs"]
mod delivery_tests;
#[cfg(test)]
#[path = "daemon_host_user_turn_delivery_tests.rs"]
mod user_turn_delivery_tests;
