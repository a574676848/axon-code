use crate::remote_commands::remote_feishu_daemon_egress::FeishuDaemonEventPublisher;
use axon_session_runtime::{EventPublisher, RuntimeClock};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
};

use super::now_ms;
use crate::daemon_commands::daemon_service_manager;

pub struct DaemonPidFileGuard {
    path: PathBuf,
    pid: u32,
}

impl DaemonPidFileGuard {
    pub fn acquire(runtime_root: &std::path::Path) -> Result<Self, String> {
        fs::create_dir_all(runtime_root)
            .map_err(|error| format!("创建 Daemon PID 目录失败: {error}"))?;
        let path = runtime_root.join(daemon_service_manager::DAEMON_PID_FILE_NAME);
        let pid = std::process::id();
        let existing_pid = fs::read_to_string(&path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok());
        if let Some(existing_pid) =
            existing_pid.filter(|pid| daemon_service_manager::process_alive(*pid))
        {
            return Err(format!("Axon Daemon 已在运行: pid={existing_pid}"));
        }
        if existing_pid.is_some() {
            let _ = fs::remove_file(&path);
        }
        fs::write(&path, format!("{pid}\n"))
            .map_err(|error| format!("写入 Daemon PID 文件失败: {error}"))?;
        Ok(Self { path, pid })
    }
}

impl Drop for DaemonPidFileGuard {
    fn drop(&mut self) {
        let matches = fs::read_to_string(&self.path)
            .ok()
            .is_some_and(|value| value.trim() == self.pid.to_string());
        if matches {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone)]
pub struct DaemonEventPublisher {
    feishu: FeishuDaemonEventPublisher,
    delivery_signal: Arc<super::DaemonMaintenanceSignal>,
}

impl DaemonEventPublisher {
    pub fn new(
        axon_root: PathBuf,
        config_path: PathBuf,
        delivery_signal: Arc<super::DaemonMaintenanceSignal>,
    ) -> Self {
        Self {
            feishu: FeishuDaemonEventPublisher::new(axon_root, config_path),
            delivery_signal,
        }
    }
}

impl EventPublisher for DaemonEventPublisher {
    fn publish(
        &self,
        event: &axon_session::SessionEventRecord,
        _cursor: &axon_domain::ConsumerCursor,
    ) -> Result<(), String> {
        if self.feishu.publish_with_work(event)? {
            self.delivery_signal.notify();
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct SystemClock;

impl RuntimeClock for SystemClock {
    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

#[derive(Clone, Copy)]
pub struct CliProcessIdentity;

impl axon_daemon::ProcessIdentity for CliProcessIdentity {
    fn pid(&self) -> u32 {
        std::process::id()
    }

    fn process_start_id(&self) -> Option<String> {
        None
    }
}

#[derive(Default)]
pub struct DaemonMaintenanceSignal {
    pub notified: Mutex<bool>,
    condition: Condvar,
}

impl DaemonMaintenanceSignal {
    /// 置位唤醒标记并唤醒一个等待者；唤醒是提示，等待者必须回到事实源复查。
    pub(crate) fn notify(&self) {
        if let Ok(mut notified) = self.notified.lock() {
            *notified = true;
            self.condition.notify_one();
        }
    }

    pub(super) fn wait(&self, timeout: std::time::Duration) {
        let Ok(mut notified) = self.notified.lock() else {
            return;
        };
        if !*notified {
            if let Ok((guard, _)) = self.condition.wait_timeout(notified, timeout) {
                notified = guard;
            } else {
                return;
            }
        }
        *notified = false;
    }
}
