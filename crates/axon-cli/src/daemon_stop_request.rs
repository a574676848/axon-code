//! 守护进程停止请求通道：调用方只写一个请求文件，守护进程自行退出。
//!
//! 守护进程由计划任务以 `HighestAvailable` 启动，而 `axon daemon stop` 通常由普通用户
//! shell 执行；跨完整性级别无法结束对方进程（`taskkill` 只会得到 Access is denied）。
//! 因此停止采用「请求退出」而不是「杀死进程」：调用方写文件，守护进程自行 drain 并退出，
//! 全程不要求调用方具备与守护进程相同的权限。计划任务结束与强制结束只作为兜底。
//!
//! 请求文件与 `daemon.pid` 同目录，这样停止与存活判据共用同一个运行态根。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 停止请求文件名。
pub(super) const DAEMON_STOP_FILE_NAME: &str = "daemon.stop";

/// 停止请求文件的完整路径。
pub(super) fn stop_request_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(DAEMON_STOP_FILE_NAME)
}

/// 写入停止请求；重复请求幂等。
///
/// 文件内容记录发起方 PID，仅用于排障，不参与判定。
pub(super) fn request(runtime_dir: &Path) -> Result<(), String> {
    let path = stop_request_path(runtime_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建守护进程停止请求目录失败: {error}"))?;
    }
    std::fs::write(&path, std::process::id().to_string())
        .map_err(|error| format!("写入守护进程停止请求失败: {error}"))
}

/// 清除停止请求。
///
/// 两个时机必须调用：启动前（避免陈旧请求立刻结束新进程）与停止收尾。
pub(super) fn clear(runtime_dir: &Path) {
    let _ = std::fs::remove_file(stop_request_path(runtime_dir));
}

/// 是否已有停止请求。
pub(super) fn is_requested(runtime_dir: &Path) -> bool {
    stop_request_path(runtime_dir).is_file()
}

/// 在后台观察停止请求，出现即置位 `stop_requested`。
///
/// 轮询间隔由调用方给出：生产用 `SERVICE_STOP_WAKEUP_POLL_MS`，测试用毫秒级。
pub(super) fn watch(
    runtime_dir: PathBuf,
    stop_requested: Arc<AtomicBool>,
    poll_interval: Duration,
) {
    std::thread::spawn(move || {
        while !stop_requested.load(Ordering::Acquire) {
            if is_requested(&runtime_dir) {
                stop_requested.store(true, Ordering::Release);
                return;
            }
            std::thread::sleep(poll_interval);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_and_clear_round_trip() {
        let dir = axon_test_support::test_temp_dir();
        let root = dir.path();
        assert!(!is_requested(root), "初始不应存在停止请求");
        request(root).expect("写入停止请求");
        assert!(is_requested(root), "写入后应判定为已请求停止");
        clear(root);
        assert!(!is_requested(root), "清除后不应再判定为已请求停止");
        // 重复清除必须幂等：停止路径可能多次收尾。
        clear(root);
    }

    #[test]
    fn watcher_flips_the_flag_when_the_request_appears() {
        let dir = axon_test_support::test_temp_dir();
        let flag = Arc::new(AtomicBool::new(false));
        watch(
            dir.path().to_path_buf(),
            Arc::clone(&flag),
            Duration::from_millis(5),
        );
        std::thread::sleep(Duration::from_millis(30));
        assert!(!flag.load(Ordering::Acquire), "无请求时不应自行置位");

        request(dir.path()).expect("写入停止请求");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !flag.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(flag.load(Ordering::Acquire), "请求出现后应立即置位停止标志");
    }
}
