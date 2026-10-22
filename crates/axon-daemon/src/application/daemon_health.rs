//! Daemon 内部健康快照组装。
//!
//! 只聚合事实源指标，不读取 Session 明细，也不暴露 Session 管理能力。

use super::{DaemonHost, DaemonHostError};
use crate::model::DaemonHostStatus;
use axon_domain::{DaemonHealthSnapshot, HealthHostState};

impl<S, E, P, C, I> DaemonHost<S, E, P, C, I>
where
    S: axon_session::RuntimeStore + Clone + 'static,
    E: Clone + 'static,
    P: Clone + 'static,
    C: axon_session_runtime::RuntimeClock + Clone + 'static,
    I: Clone + 'static,
{
    /// 读取当前 Host 的内部健康快照。
    pub fn health_snapshot(&self) -> Result<DaemonHealthSnapshot, DaemonHostError> {
        let now_ms = self.clock.now_ms();
        let counters = self.store.read_runtime_health(now_ms)?;
        // lease 以最近一次成功续租写入的绝对到期时间报告；start() 之前的未启动状态没有租约。
        let lease_expires_at_ms = self
            .lease_expires_at_ms
            .load(std::sync::atomic::Ordering::Acquire);
        let (host_id, generation, status) = match self.host.as_ref() {
            Some(host) => (
                host.host_id.as_str().to_owned(),
                host.generation.clone(),
                self.status(),
            ),
            None => (
                self.host_id.as_str().to_owned(),
                String::new(),
                self.status(),
            ),
        };
        let deliveries = self
            .delivery_health
            .as_ref()
            .map(|probe| probe.read_delivery_health(now_ms))
            .transpose()
            .map_err(DaemonHostError::Worker)?;
        Ok(DaemonHealthSnapshot {
            generated_at_ms: now_ms,
            host: HealthHostState {
                host_id,
                generation,
                status: host_status_text(status).to_owned(),
                lease_expires_at_ms,
                lease_remaining_ms: lease_expires_at_ms.saturating_sub(now_ms),
            },
            interactions: counters.interactions,
            goals: counters.goals,
            executions: counters.executions,
            execution_backlog: counters.execution_backlog,
            deliveries,
            stream_subscribers: u64::try_from(self.stream_bridge.subscriber_count())
                .unwrap_or(u64::MAX),
        })
    }
}

fn host_status_text(status: DaemonHostStatus) -> &'static str {
    match status {
        DaemonHostStatus::Running => "running",
        DaemonHostStatus::UpgradeRequested => "upgrade_requested",
        DaemonHostStatus::AdmittingShutdown => "admitting_shutdown",
        DaemonHostStatus::HandoffReady => "handoff_ready",
        DaemonHostStatus::Stopping => "stopping",
        DaemonHostStatus::Stopped => "stopped",
    }
}
