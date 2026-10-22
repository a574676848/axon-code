//! Worker Supervisor 端口模型。

use axon_domain::DaemonSessionId;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, PoisonError},
};

/// Worker supervisor 的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerSupervisorState {
    /// 正常接收并执行任务。
    Running,
    /// 停止接收新任务，等待已领取任务完成。
    Draining,
    /// Supervisor 已停止。
    Stopped,
}

#[derive(Debug, Default)]
pub(super) struct ExecutionWorkerRegistry {
    state: Mutex<ExecutionWorkerRegistryState>,
}

#[derive(Debug, Default)]
struct ExecutionWorkerRegistryState {
    active: HashSet<DaemonSessionId>,
    pending: HashSet<DaemonSessionId>,
}

impl ExecutionWorkerRegistry {
    pub(super) fn register(
        self: &Arc<Self>,
        session_id: DaemonSessionId,
    ) -> Option<ExecutionWorkerLease> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.active.insert(session_id.clone()) {
            state.pending.insert(session_id);
            return None;
        }
        state.pending.remove(&session_id);
        Some(ExecutionWorkerLease {
            registry: Arc::clone(self),
            session_id,
            owns_registration: true,
        })
    }

    /// 当前登记的活跃与待唤醒 Session 数量；常驻 soak 以此判定无持续增长。
    #[cfg(test)]
    pub(super) fn tracked_sessions(&self) -> usize {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.active.len().saturating_add(state.pending.len())
    }

    #[cfg(test)]
    pub(super) fn active_sessions(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .len()
    }

    #[cfg(test)]
    pub(super) fn pending_sessions(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .len()
    }

    fn release(&self, session_id: &DaemonSessionId) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .active
            .remove(session_id);
    }
}

pub(super) struct ExecutionWorkerLease {
    registry: Arc<ExecutionWorkerRegistry>,
    session_id: DaemonSessionId,
    owns_registration: bool,
}

impl ExecutionWorkerLease {
    pub(super) fn continue_if_pending(&mut self) -> bool {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.pending.remove(&self.session_id) {
            return true;
        }
        state.active.remove(&self.session_id);
        self.owns_registration = false;
        false
    }
}

impl Drop for ExecutionWorkerLease {
    fn drop(&mut self) {
        if self.owns_registration {
            self.registry.release(&self.session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_unwind_releases_registration_and_preserves_pending_wake() {
        let registry = Arc::new(ExecutionWorkerRegistry::default());
        let session_id = DaemonSessionId::new("worker-registry-session").expect("Session ID");
        let registration = registry
            .register(session_id.clone())
            .expect("首次注册 Worker");

        assert!(registry.register(session_id.clone()).is_none());
        drop(registration);

        let mut replacement = registry.register(session_id).expect("接管 pending 唤醒");
        assert!(!replacement.continue_if_pending());
    }
}
