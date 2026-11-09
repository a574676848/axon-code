use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process,
};

use axon_crypto::sha256_hex;
use serde::{Deserialize, Serialize};

use crate::daemon_commands::daemon_process_alive;

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
            // 判据必须是「该 PID 仍由 Axon 进程持有」：PID 被系统回收给无关进程后，
            // 只看存活会让陈旧锁文件永久拒绝启动，Feishu 远控随之卡在 failed。
            if existing.pid != pid && daemon_process_alive(existing.pid) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// PID 被系统回收给无关进程时，陈旧锁文件不得继续阻塞 gateway 启动。
    #[cfg(windows)]
    #[test]
    fn acquire_reclaims_lock_held_by_a_recycled_pid() {
        let directory = axon_test_support::test_temp_dir();
        let root = directory.path();
        let path = lock_path(root, "cli_recycled_pid");
        fs::create_dir_all(path.parent().expect("锁文件父目录")).expect("创建锁目录");
        let mut foreign = std::process::Command::new("cmd")
            .args(["/c", "ping -n 30 127.0.0.1 > nul"])
            .spawn()
            .expect("启动无关进程");
        let stale = FeishuAppLockRecord {
            pid: foreign.id(),
            app_id_sha256: sha256_hex("cli_recycled_pid"),
            started_at_ms: 1,
        };
        fs::write(&path, serde_json::to_string(&stale).expect("序列化陈旧锁")).expect("写入陈旧锁");

        let guard =
            FeishuAppLockGuard::acquire(root, "cli_recycled_pid", 42).expect("接管被回收的 PID");
        let record = read_lock(&path).expect("读取锁").expect("锁应存在");
        assert_eq!(record.pid, std::process::id());
        drop(guard);
        let _ = foreign.kill();
        let _ = foreign.wait();
    }

    #[test]
    fn acquire_reclaims_lock_left_by_a_dead_process() {
        let directory = axon_test_support::test_temp_dir();
        let root = directory.path();
        let path = lock_path(root, "cli_dead_pid");
        fs::create_dir_all(path.parent().expect("锁文件父目录")).expect("创建锁目录");
        let stale = FeishuAppLockRecord {
            pid: u32::MAX,
            app_id_sha256: sha256_hex("cli_dead_pid"),
            started_at_ms: 1,
        };
        fs::write(&path, serde_json::to_string(&stale).expect("序列化陈旧锁")).expect("写入陈旧锁");

        let guard =
            FeishuAppLockGuard::acquire(root, "cli_dead_pid", 42).expect("接管已退出进程的锁");
        let record = read_lock(&path).expect("读取锁").expect("锁应存在");
        assert_eq!(record.pid, std::process::id());
        drop(guard);
    }
}
