//! Memory 本机投影 sidecar 文件锁。

use std::{
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    thread,
    time::{Duration, SystemTime},
};

use axon_domain::{AxonError, AxonResult};

/// Memory 文件锁获取超时，单位毫秒。
pub const MEMORY_FILE_LOCK_TIMEOUT_MS: u64 = 5_000;
/// Memory 文件锁轮询间隔，单位毫秒。
const MEMORY_FILE_LOCK_POLL_MS: u64 = 25;
/// Memory 文件锁过期清理阈值，单位毫秒。
const MEMORY_FILE_LOCK_STALE_MS: u64 = 900_000;
/// Memory sidecar lock 文件后缀。
const MEMORY_FILE_LOCK_SUFFIX: &str = "lock";

/// 基于 sidecar 文件的跨线程/跨进程互斥锁。
pub struct MemoryFileLock {
    path: PathBuf,
}

impl MemoryFileLock {
    /// 为目标文件获取 sidecar 写锁。
    pub fn acquire(target_path: &Path) -> AxonResult<Self> {
        let lock_path = lock_path(target_path)?;
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)
                .map_err(|_| AxonError::memory_persistence("无法创建 Memory 文件锁目录"))?;
        }
        let started = SystemTime::now();
        loop {
            match try_create_lock(&lock_path) {
                Ok(()) => return Ok(Self { path: lock_path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    clear_stale_lock(&lock_path)?;
                    if elapsed_ms(started) >= MEMORY_FILE_LOCK_TIMEOUT_MS {
                        return Err(AxonError::memory_persistence("Memory 文件锁获取超时"));
                    }
                    thread::sleep(Duration::from_millis(MEMORY_FILE_LOCK_POLL_MS));
                }
                Err(_) => {
                    return Err(AxonError::memory_persistence("Memory 文件锁创建失败"));
                }
            }
        }
    }
}

impl Drop for MemoryFileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn try_create_lock(path: &Path) -> std::io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    write_lock_owner(&mut file)
}

fn write_lock_owner(file: &mut File) -> std::io::Result<()> {
    file.write_all(std::process::id().to_string().as_bytes())?;
    file.sync_all()
}

fn clear_stale_lock(path: &Path) -> AxonResult<()> {
    let Ok(metadata) = fs::metadata(path) else {
        return Ok(());
    };
    let Ok(modified) = metadata.modified() else {
        return Ok(());
    };
    if elapsed_ms(modified) >= MEMORY_FILE_LOCK_STALE_MS {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(AxonError::memory_persistence("Memory 过期文件锁清理失败"));
            }
        }
    }
    Ok(())
}

fn elapsed_ms(started: SystemTime) -> u64 {
    started
        .elapsed()
        .map_or(0, |elapsed| elapsed.as_millis())
        .try_into()
        .unwrap_or(u64::MAX)
}

fn lock_path(target_path: &Path) -> AxonResult<PathBuf> {
    let file_name = target_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(AxonError::memory_persistence("Memory 文件锁目标名无效"))?;
    Ok(target_path.with_file_name(format!("{file_name}.{MEMORY_FILE_LOCK_SUFFIX}")))
}
