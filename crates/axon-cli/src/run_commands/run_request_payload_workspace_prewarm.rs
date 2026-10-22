//! Run 生命周期内的 Workspace 快照预热任务。

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use axon_session_runtime::TurnPerformanceRecorder;

use super::{WORKSPACE_PREWARM_TASK, WorkspaceSnapshotResult};

pub(crate) struct RunWorkspaceSnapshotPrewarm {
    handle: Mutex<Option<thread::JoinHandle<WorkspaceSnapshotResult>>>,
}

impl RunWorkspaceSnapshotPrewarm {
    pub(in crate::run_commands) fn spawn(
        config_path: PathBuf,
        cwd: PathBuf,
        performance: Option<Arc<dyn TurnPerformanceRecorder>>,
    ) -> Self {
        let handle = thread::spawn(move || {
            super::resolve_run_workspace_snapshot_with_telemetry(
                &config_path,
                &cwd,
                performance.as_deref(),
            )
        });
        Self {
            handle: Mutex::new(Some(handle)),
        }
    }

    pub(in crate::run_commands) fn resolve(&self) -> WorkspaceSnapshotResult {
        let handle = self
            .handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .ok_or_else(|| format!("{WORKSPACE_PREWARM_TASK}结果已消费"))?;
        handle
            .join()
            .map_err(|_| format!("{WORKSPACE_PREWARM_TASK}任务异常退出"))?
    }
}

impl Drop for RunWorkspaceSnapshotPrewarm {
    fn drop(&mut self) {
        let handle = self
            .handle
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}
