use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

const GATEWAY_STATUS_FILE_NAME: &str = "gateway-status.json";
const GATEWAY_STATUS_TEMP_SUFFIX: &str = "tmp";
const GATEWAY_STATUS_ERROR_LIMIT: usize = 512;

// Status updates read-modify-write one shared file from several gateway threads.
static GATEWAY_STATUS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum GatewayRuntimeState {
    Starting,
    Connecting,
    Connected,
    Reconnecting,
    Draining,
    Drained,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GatewayRuntimeStatus {
    pub(super) state: GatewayRuntimeState,
    pub(super) pid: u32,
    pub(super) started_at_ms: u64,
    pub(super) updated_at_ms: u64,
    pub(super) reconnect_attempts: usize,
    pub(super) last_error: Option<String>,
    #[serde(default)]
    pub(super) active_task_count: usize,
}

impl GatewayRuntimeStatus {
    pub(super) fn transition(
        runtime_root: &Path,
        pid: u32,
        state: GatewayRuntimeState,
        reconnect_attempts: Option<usize>,
        last_error: Option<&str>,
    ) -> Result<Self, String> {
        let _lock = GATEWAY_STATUS_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = gateway_status_path(runtime_root);
        let now_ms = axon_time::unix_now_ms();
        let previous = Self::read(&path).filter(|status| status.pid == pid);
        let started_at_ms = previous
            .as_ref()
            .map_or(now_ms, |status| status.started_at_ms);
        let reconnect_attempts = reconnect_attempts
            .or_else(|| previous.as_ref().map(|status| status.reconnect_attempts));
        let status = Self {
            state,
            pid,
            started_at_ms,
            updated_at_ms: now_ms,
            reconnect_attempts: reconnect_attempts.unwrap_or_default(),
            last_error: last_error.map(bounded_error),
            active_task_count: previous.map_or(0, |status| status.active_task_count),
        };
        write_status(&path, &status)?;
        Ok(status)
    }

    pub(super) fn read(path: &Path) -> Option<Self> {
        let content = fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    pub(super) fn update_active_task_count(
        runtime_root: &Path,
        pid: u32,
        active_task_count: usize,
    ) -> Result<Self, String> {
        let _lock = GATEWAY_STATUS_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let path = gateway_status_path(runtime_root);
        let mut status = Self::read(&path)
            .filter(|status| status.pid == pid)
            .unwrap_or_else(|| Self {
                state: GatewayRuntimeState::Starting,
                pid,
                started_at_ms: axon_time::unix_now_ms(),
                updated_at_ms: axon_time::unix_now_ms(),
                reconnect_attempts: 0,
                last_error: None,
                active_task_count: 0,
            });
        status.updated_at_ms = axon_time::unix_now_ms();
        status.active_task_count = active_task_count;
        write_status(&path, &status)?;
        Ok(status)
    }
}

pub(super) fn gateway_status_path(runtime_root: &Path) -> PathBuf {
    runtime_root.join(GATEWAY_STATUS_FILE_NAME)
}

fn write_status(path: &Path, status: &GatewayRuntimeStatus) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("创建 gateway 状态目录失败: {error}"))?;
    }
    let payload =
        serde_json::to_vec(status).map_err(|error| format!("序列化 gateway 状态失败: {error}"))?;
    let temp = path.with_extension(format!(
        "json.{}.{}",
        std::process::id(),
        GATEWAY_STATUS_TEMP_SUFFIX
    ));
    {
        let mut file = fs::File::create(&temp)
            .map_err(|error| format!("创建 gateway 临时状态失败: {error}"))?;
        file.write_all(&payload)
            .map_err(|error| format!("写入 gateway 临时状态失败: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("同步 gateway 临时状态失败: {error}"))?;
    }
    fs::rename(&temp, path).map_err(|error| format!("替换 gateway 状态失败: {error}"))
}

fn bounded_error(error: &str) -> String {
    let error = error.trim();
    if error.chars().count() <= GATEWAY_STATUS_ERROR_LIMIT {
        return error.to_owned();
    }
    let mut bounded = error
        .chars()
        .take(GATEWAY_STATUS_ERROR_LIMIT)
        .collect::<String>();
    bounded.push_str("...");
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_transition_preserves_start_time_and_bounds_error() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-gateway-status-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let starting = GatewayRuntimeStatus::transition(
            &root,
            7,
            GatewayRuntimeState::Starting,
            Some(0),
            None,
        )
        .expect("starting 状态应可写入");
        let reconnecting = GatewayRuntimeStatus::transition(
            &root,
            7,
            GatewayRuntimeState::Reconnecting,
            Some(2),
            Some(&"x".repeat(GATEWAY_STATUS_ERROR_LIMIT + 10)),
        )
        .expect("reconnecting 状态应可写入");

        assert_eq!(reconnecting.started_at_ms, starting.started_at_ms);
        assert_eq!(reconnecting.reconnect_attempts, 2);
        let active = GatewayRuntimeStatus::update_active_task_count(&root, 7, 1)
            .expect("活动任务数应可更新");
        assert_eq!(active.active_task_count, 1);
        assert_eq!(active.state, GatewayRuntimeState::Reconnecting);
        assert_eq!(
            active.last_error.as_deref().map(str::len),
            Some(GATEWAY_STATUS_ERROR_LIMIT + 3)
        );
        assert_eq!(
            GatewayRuntimeStatus::read(&gateway_status_path(&root)),
            Some(active)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_status_updates_are_serialized() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-gateway-status-concurrent-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let root = std::sync::Arc::new(root);
        let start = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles = (0..8)
            .map(|worker| {
                let root = std::sync::Arc::clone(&root);
                let start = std::sync::Arc::clone(&start);
                std::thread::spawn(move || {
                    start.wait();
                    for attempt in 0..64 {
                        GatewayRuntimeStatus::transition(
                            &root,
                            7,
                            GatewayRuntimeState::Reconnecting,
                            Some(worker * 64 + attempt),
                            None,
                        )
                        .expect("并发状态 transition 不应失败");
                        GatewayRuntimeStatus::update_active_task_count(&root, 7, attempt % 2)
                            .expect("并发 active task 更新不应失败");
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("状态更新线程不应 panic");
        }
        assert!(GatewayRuntimeStatus::read(&gateway_status_path(&root)).is_some());
        let _ = fs::remove_dir_all(root.as_ref());
    }
}
