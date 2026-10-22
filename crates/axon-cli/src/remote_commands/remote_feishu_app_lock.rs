use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process,
};

#[cfg(not(windows))]
use std::process::Stdio;

use axon_crypto::sha256_hex;
#[cfg(windows)]
use axon_process::non_interactive_command;
use serde::{Deserialize, Serialize};

const FEISHU_APP_LOCK_DIR: &str = "app_locks";

#[derive(Debug)]
pub(super) struct FeishuAppLockGuard {
    path: PathBuf,
    pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FeishuAppLockRecord {
    pid: u32,
    app_id_sha256: String,
    started_at_ms: u64,
}

impl FeishuAppLockGuard {
    pub(super) fn acquire(root: &Path, app_id: &str, started_at_ms: u64) -> Result<Self, String> {
        let app_id = app_id.trim();
        if app_id.is_empty() {
            return Err("Feishu app lock 缺少 app_id".to_owned());
        }
        let pid = process::id();
        let path = lock_path(root, app_id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("创建 Feishu app lock 目录失败: {error}"))?;
        }
        if let Some(existing) = read_lock(&path)? {
            if existing.pid != pid && process_alive(existing.pid) {
                return Err(format!(
                    "Feishu app_id 已被本机其他 gateway 使用，pid={}",
                    existing.pid
                ));
            }
            let _ = fs::remove_file(&path);
        }
        let record = FeishuAppLockRecord {
            pid,
            app_id_sha256: sha256_hex(app_id),
            started_at_ms,
        };
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("获取 Feishu app lock 失败: {error}"))?;
        serde_json::to_writer(file, &record)
            .map_err(|error| format!("写入 Feishu app lock 失败: {error}"))?;
        Ok(Self { path, pid })
    }
}

impl Drop for FeishuAppLockGuard {
    fn drop(&mut self) {
        if read_lock(&self.path)
            .ok()
            .flatten()
            .is_some_and(|record| record.pid == self.pid)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn lock_path(root: &Path, app_id: &str) -> PathBuf {
    root.join(FEISHU_APP_LOCK_DIR)
        .join(format!("{}.json", sha256_hex(app_id)))
}

fn read_lock(path: &Path) -> Result<Option<FeishuAppLockRecord>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let content =
        fs::read_to_string(path).map_err(|error| format!("读取 Feishu app lock 失败: {error}"))?;
    if content.trim().is_empty() {
        return Ok(None);
    }
    serde_json::from_str::<FeishuAppLockRecord>(&content)
        .map(Some)
        .map_err(|error| format!("Feishu app lock JSON 无效: {error}"))
}

fn process_alive(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }
    #[cfg(windows)]
    {
        non_interactive_command("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .is_some_and(|stdout| stdout.contains(&pid.to_string()))
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::process_alive;

    #[test]
    fn process_alive_recognizes_current_process() {
        assert!(process_alive(std::process::id()));
    }

    #[cfg(not(windows))]
    #[test]
    fn process_alive_rejects_missing_pid() {
        assert!(!process_alive(u32::MAX));
    }
}
