//! Daemon Host 生命周期状态的紧凑编码。

use crate::model::DaemonHostStatus;

const HOST_STATUS_STOPPED: u8 = 0;
const HOST_STATUS_RUNNING: u8 = 1;
const HOST_STATUS_ADMITTING_SHUTDOWN: u8 = 2;
const HOST_STATUS_HANDOFF_READY: u8 = 3;

pub(super) fn encode(status: DaemonHostStatus) -> u8 {
    match status {
        DaemonHostStatus::Running => HOST_STATUS_RUNNING,
        DaemonHostStatus::UpgradeRequested | DaemonHostStatus::AdmittingShutdown => {
            HOST_STATUS_ADMITTING_SHUTDOWN
        }
        DaemonHostStatus::HandoffReady => HOST_STATUS_HANDOFF_READY,
        DaemonHostStatus::Stopping | DaemonHostStatus::Stopped => HOST_STATUS_STOPPED,
    }
}

pub(super) fn decode(code: u8) -> DaemonHostStatus {
    match code {
        HOST_STATUS_RUNNING => DaemonHostStatus::Running,
        HOST_STATUS_ADMITTING_SHUTDOWN => DaemonHostStatus::AdmittingShutdown,
        HOST_STATUS_HANDOFF_READY => DaemonHostStatus::HandoffReady,
        _ => DaemonHostStatus::Stopped,
    }
}
