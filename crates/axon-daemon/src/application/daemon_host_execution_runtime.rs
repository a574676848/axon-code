//! Daemon Host 的 Session Runtime 生命周期扩展。

use super::{DaemonHost, DaemonHostError};
use crate::ports::ProcessIdentity;
use axon_domain::DaemonSessionId;
use axon_session::{RuntimeStore, SessionRuntimeRecord};
use axon_session_runtime::{EventPublisher, ExecutionWorkerError, RuntimeClock, TurnExecutor};

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: RuntimeStore + Clone + 'static,
    E: TurnExecutor + Clone + 'static,
    P: EventPublisher + Clone + 'static,
    C: RuntimeClock + Clone + 'static,
    I: ProcessIdentity + Clone + 'static,
{
    /// 停止 Session 当前运行，并取消所有未完成 Action。
    pub fn stop_session(
        &self,
        session_id: &DaemonSessionId,
    ) -> Result<SessionRuntimeRecord, DaemonHostError> {
        let host = self.started_host()?;
        self.store
            .stop_session(session_id, host, self.clock.now_ms())
            .map_err(Into::into)
    }

    /// 原子停止旧 rollout，并为同一 transport Session 创建新的 rollout 身份。
    pub fn reset_session(
        &self,
        session_id: &DaemonSessionId,
        cwd: &str,
    ) -> Result<SessionRuntimeRecord, DaemonHostError> {
        let host = self.started_host()?;
        self.store
            .reset_session(session_id, cwd, host, self.clock.now_ms())
            .map_err(Into::into)
    }
}

pub(super) fn map_execution_worker_error(error: ExecutionWorkerError) -> DaemonHostError {
    match error {
        ExecutionWorkerError::Store(error) => DaemonHostError::Store(error),
        ExecutionWorkerError::Execution(error) | ExecutionWorkerError::Publish(error) => {
            DaemonHostError::Worker(error)
        }
        ExecutionWorkerError::Gate(error) => DaemonHostError::Worker(error),
    }
}
