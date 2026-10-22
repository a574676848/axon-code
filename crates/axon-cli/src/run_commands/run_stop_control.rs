//! `axon stop` 与正在运行的 `axon run` 之间的本机取消协调。

use std::{
    fs::{self, OpenOptions},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde::Serialize;

use crate::daemon_commands::process_alive;
use crate::runtime_paths::runtime_axon_data_path_static;

use super::RunCancelToken;

const RUN_CONTROL_DIRECTORY: &str = "run-control";
const ACTIVE_SUFFIX: &str = ".active";
const STOP_SUFFIX: &str = ".stop";
/// 启动恢复只做有界清理，异常膨胀的目录留给逐次 stop 选择路径继续收敛。
const STARTUP_ACTIVE_SCAN_LIMIT: usize = 64;
const SESSION_ID_HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StopRequestReceipt {
    pub(crate) session_id: String,
    pub(crate) status: &'static str,
    pub(crate) message: String,
}

/// 当前 CLI run 的取消监听器。Drop 时清理自身控制文件。
pub(crate) struct RunStopControl {
    active: Arc<AtomicBool>,
    active_path: PathBuf,
    stop_path: PathBuf,
    watcher: Option<JoinHandle<()>>,
}

impl RunStopControl {
    pub(crate) fn start(
        config_path: &Path,
        session_id: &str,
        cancel_token: RunCancelToken,
    ) -> Result<Self, String> {
        let paths = control_paths(config_path, session_id)?;
        fs::create_dir_all(&paths.directory)
            .map_err(|_| "无法创建 run stop 控制目录".to_owned())?;
        let pending_stop = paths.stop_path.exists();
        if paths.active_path.exists() && !remove_stale_active_file(&paths.active_path)? {
            return Err("同一 Session 的 Axon run 仍在运行".to_owned());
        }
        fs::write(&paths.active_path, std::process::id().to_string())
            .map_err(|_| "无法创建活动 run 控制记录".to_owned())?;

        let active = Arc::new(AtomicBool::new(true));
        if pending_stop {
            cancel_token.cancel();
        }
        let watcher_active = Arc::clone(&active);
        let stop_path = paths.stop_path.clone();
        let config_path = config_path.to_path_buf();
        let session_id = session_id.to_owned();
        let watcher = thread::spawn(move || {
            while watcher_active.load(Ordering::Acquire) {
                if stop_path.exists() {
                    if let Err(error) = super::run_thread_store::mirror_session_terminal(
                        &config_path,
                        &session_id,
                        "interrupted",
                        None,
                    ) {
                        axon_logger::warning(&format!(
                            "stop thread terminal persistence skipped: session_id={session_id}, error={error}"
                        ));
                    }
                    cancel_token.cancel();
                    break;
                }
                thread::sleep(STOP_POLL_INTERVAL);
            }
        });

        Ok(Self {
            active,
            active_path: paths.active_path,
            stop_path: paths.stop_path,
            watcher: Some(watcher),
        })
    }
}

/// Daemon 启动时清理进程已不存在的 run 控制文件，避免旧崩溃阻塞新的 stop/continue。
pub(crate) fn cleanup_stale_run_controls(config_path: &Path) -> Result<u32, String> {
    let directory = control_directory(config_path)?;
    if !directory.exists() {
        return Ok(0);
    }
    let entries = fs::read_dir(&directory)
        .map_err(|error| format!("读取 run 控制目录失败: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| decode_control_session_id(&name, ACTIVE_SUFFIX))
        .take(STARTUP_ACTIVE_SCAN_LIMIT)
        .map(|session_id| {
            let active_path =
                directory.join(format!("{}{ACTIVE_SUFFIX}", encode_session_id(&session_id)));
            if remove_stale_active_file(&active_path)? {
                Ok(1_u32)
            } else {
                Ok(0_u32)
            }
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(entries.iter().sum())
}

impl Drop for RunStopControl {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        let _ = fs::remove_file(&self.active_path);
        let _ = fs::remove_file(&self.stop_path);
    }
}

/// 请求停止一个正在运行的 CLI session；重复请求保持幂等。
pub(crate) fn request_stop(
    config_path: &Path,
    session_id: Option<&str>,
) -> Result<StopRequestReceipt, String> {
    let directory = control_directory(config_path)?;
    let session_id = select_active_session(&directory, session_id)?;
    request_stop_for_session(&directory, &session_id)
}

fn request_stop_for_session(
    directory: &Path,
    session_id: &str,
) -> Result<StopRequestReceipt, String> {
    let stop_path = control_paths_for_directory(directory, session_id)?.stop_path;
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&stop_path)
    {
        Ok(_) => Ok(StopRequestReceipt {
            session_id: session_id.to_owned(),
            status: "requested",
            message: "已请求停止；运行进程会持久化中断状态，可使用 `axon run --continue` 恢复"
                .to_owned(),
        }),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(StopRequestReceipt {
            session_id: session_id.to_owned(),
            status: "already_requested",
            message: "停止请求已存在，正在等待运行进程完成取消".to_owned(),
        }),
        Err(_) => Err("无法写入 run stop 请求".to_owned()),
    }
}

struct ControlPaths {
    directory: PathBuf,
    active_path: PathBuf,
    stop_path: PathBuf,
}

fn control_paths(config_path: &Path, session_id: &str) -> Result<ControlPaths, String> {
    control_paths_for_directory(&control_directory(config_path)?, session_id)
}

fn control_paths_for_directory(directory: &Path, session_id: &str) -> Result<ControlPaths, String> {
    validate_session_id(session_id)?;
    let encoded_session_id = encode_session_id(session_id);
    Ok(ControlPaths {
        directory: directory.to_path_buf(),
        active_path: directory.join(format!("{encoded_session_id}{ACTIVE_SUFFIX}")),
        stop_path: directory.join(format!("{encoded_session_id}{STOP_SUFFIX}")),
    })
}

fn encode_session_id(session_id: &str) -> String {
    let mut encoded = String::with_capacity(session_id.len().saturating_mul(2));
    for byte in session_id.as_bytes() {
        encoded.push(char::from(SESSION_ID_HEX_DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(SESSION_ID_HEX_DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_control_session_id(file_name: &str, suffix: &str) -> Option<String> {
    let encoded = file_name.strip_suffix(suffix)?;
    if encoded.len() % 2 != 0 {
        return None;
    }
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.as_chunks::<2>().0 {
        let high = char::from(pair[0]).to_digit(16)?;
        let low = char::from(pair[1]).to_digit(16)?;
        decoded.push(u8::try_from((high << 4) | low).ok()?);
    }
    String::from_utf8(decoded).ok()
}

fn control_directory(config_path: &Path) -> Result<PathBuf, String> {
    runtime_axon_data_path_static(config_path)
        .map(|root| root.join(RUN_CONTROL_DIRECTORY))
        .map_err(ToOwned::to_owned)
}

fn select_active_session(directory: &Path, requested: Option<&str>) -> Result<String, String> {
    if let Some(session_id) = requested {
        let paths = control_paths_for_directory(directory, session_id)?;
        if paths.active_path.exists() && !remove_stale_active_file(&paths.active_path)? {
            return Ok(session_id.to_owned());
        }
        return Err(format!("没有正在运行的 Axon session：{session_id}"));
    }
    let entries = fs::read_dir(directory).map_err(|_| "没有正在运行的 Axon session".to_owned())?;
    let candidate_names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| decode_control_session_id(&name, ACTIVE_SUFFIX))
        .take(STARTUP_ACTIVE_SCAN_LIMIT)
        .collect::<Vec<_>>();
    let mut sessions = Vec::with_capacity(candidate_names.len());
    for name in candidate_names {
        let active_path = directory.join(format!("{}{ACTIVE_SUFFIX}", encode_session_id(&name)));
        if active_path.exists() && !remove_stale_active_file(&active_path)? {
            sessions.push(name);
        }
    }
    match sessions.as_slice() {
        [] => Err("没有正在运行的 Axon session".to_owned()),
        [session_id] => Ok(session_id.clone()),
        _ => Err("存在多个正在运行的 Axon session；请使用 `axon stop --session <id>`".to_owned()),
    }
}

/// 返回 true 表示 stale 文件已清理；false 表示 PID 仍存活。
fn remove_stale_active_file(active_path: &Path) -> Result<bool, String> {
    let contents = fs::read_to_string(active_path)
        .map_err(|error| format!("无法读取 run 控制文件 PID: {error}"))?;
    let pid = contents
        .trim()
        .parse::<u32>()
        .map_err(|_| "run 控制文件 PID 无效".to_owned())?;
    if process_alive(pid) {
        return Ok(false);
    }
    fs::remove_file(active_path)
        .map_err(|error| format!("清理 stale run 控制文件失败: {error}"))?;
    Ok(true)
}

fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty() || session_id.contains(['/', '\\']) {
        return Err("session ID 不能包含路径分隔符".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_cleanup_removes_only_dead_active_controls() {
        let directory = axon_test_support::test_temp_root()
            .join(format!("axon-stop-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            control_paths_for_directory(&directory, "stale-run")
                .unwrap()
                .active_path,
            u32::MAX.to_string(),
        )
        .unwrap();

        assert!(
            remove_stale_active_file(
                &control_paths_for_directory(&directory, "stale-run")
                    .unwrap()
                    .active_path
            )
            .unwrap()
        );
        assert!(
            !control_paths_for_directory(&directory, "stale-run")
                .unwrap()
                .active_path
                .exists()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn active_selection_ignores_dead_owners_and_keeps_live_owners() {
        let directory = axon_test_support::test_temp_root()
            .join(format!("axon-stop-select-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            control_paths_for_directory(&directory, "stale-run")
                .unwrap()
                .active_path,
            u32::MAX.to_string(),
        )
        .unwrap();
        assert!(select_active_session(&directory, None).is_err());
        assert!(
            !control_paths_for_directory(&directory, "stale-run")
                .unwrap()
                .active_path
                .exists()
        );

        fs::write(
            control_paths_for_directory(&directory, "live-run")
                .unwrap()
                .active_path,
            std::process::id().to_string(),
        )
        .unwrap();
        assert_eq!(select_active_session(&directory, None).unwrap(), "live-run");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn stop_tree_targets_only_root_and_descendants() {
        let directory = axon_test_support::test_temp_root()
            .join(format!("axon-stop-control-tree-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        for session_id in [
            "root",
            "root::subagent::child-1",
            "root-other::subagent::child-2",
        ] {
            let path = control_paths_for_directory(&directory, session_id)
                .unwrap()
                .active_path;
            fs::write(path, "1").unwrap();
        }
        let descendant_prefix = "root::subagent::";
        let mut sessions = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter_map(|name| decode_control_session_id(&name, ACTIVE_SUFFIX))
            .filter(|session_id| session_id == "root" || session_id.starts_with(descendant_prefix))
            .collect::<Vec<_>>();
        sessions.sort_unstable();
        let receipts = sessions
            .iter()
            .map(|session_id| request_stop_for_session(&directory, session_id).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(
            receipts
                .iter()
                .map(|receipt| receipt.session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["root", "root::subagent::child-1"]
        );
        assert!(
            !control_paths_for_directory(&directory, "root-other::subagent::child-2")
                .unwrap()
                .stop_path
                .exists()
        );
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn stop_request_targets_the_only_active_session() {
        let directory = axon_test_support::test_temp_root()
            .join(format!("axon-stop-control-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            control_paths_for_directory(&directory, "run-1")
                .unwrap()
                .active_path,
            "1",
        )
        .unwrap();
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn multiple_active_sessions_require_an_explicit_selector() {
        let directory = axon_test_support::test_temp_root()
            .join(format!("axon-stop-control-multiple-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        for session_id in ["run-1", "run-2"] {
            fs::write(
                control_paths_for_directory(&directory, session_id)
                    .unwrap()
                    .active_path,
                std::process::id().to_string(),
            )
            .unwrap();
        }
        let error = select_active_session(&directory, None).unwrap_err();

        assert!(error.contains("--session"));
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn control_directory_is_under_runtime_data_directory() {
        let config_path = Path::new("C:/axon/config.toml");

        assert_eq!(
            control_directory(config_path).unwrap(),
            PathBuf::from("C:/axon/data/run-control")
        );
    }
}
