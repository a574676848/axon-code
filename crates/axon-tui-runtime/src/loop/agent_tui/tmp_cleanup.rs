//! Agent TUI 用户级临时目录清理。

use std::{
    fs::{self, OpenOptions},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime},
};

use axon_agent_runtime::BackgroundScheduler;

use super::clipboard::CLIPBOARD_IMAGE_FILE_PREFIX;

const TMP_CLEANUP_INTERVAL_MS: u64 = 15 * 60 * 1000;
const TMP_FILE_TTL: Duration = Duration::from_hours(24);
const PNG_EXTENSION: &str = "png";
const TMP_CLEANUP_LOCK_FILE: &str = "agent-tui-tmp-cleanup.lock";

static GLOBAL_TMP_CLEANUP: OnceLock<Mutex<Option<BackgroundScheduler<AgentTuiTmpCleanupTick>>>> =
    OnceLock::new();

/// TUI 临时文件清理 tick 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiTmpCleanupTick {
    /// 被清理的文件数。
    pub removed_files: usize,
}

/// 确保 TUI 临时目录清理调度已启动。
pub fn ensure_agent_tui_tmp_cleanup_started(tmp_dir: PathBuf) {
    let scheduler = GLOBAL_TMP_CLEANUP.get_or_init(|| Mutex::new(None));
    let Ok(mut guard) = scheduler.lock() else {
        return;
    };
    if guard.is_some() {
        return;
    }
    *guard = Some(BackgroundScheduler::start(
        TMP_CLEANUP_INTERVAL_MS,
        move || {
            let lock_path = tmp_dir.join(TMP_CLEANUP_LOCK_FILE);
            let _lock = HostCleanupLock::try_acquire(&lock_path)?;
            cleanup_agent_tui_tmp_dir(&tmp_dir)
                .ok()
                .map(|removed_files| AgentTuiTmpCleanupTick { removed_files })
        },
    ));
}

/// 清理 Axon TUI 自己写入的过期临时文件。
pub fn cleanup_agent_tui_tmp_dir(tmp_dir: &Path) -> io::Result<usize> {
    cleanup_agent_tui_tmp_dir_at(tmp_dir, SystemTime::now())
}

fn cleanup_agent_tui_tmp_dir_at(tmp_dir: &Path, now: SystemTime) -> io::Result<usize> {
    let Ok(entries) = fs::read_dir(tmp_dir) else {
        return Ok(0);
    };
    let mut removed = 0usize;
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if !is_expired_clipboard_image(&path, now)? {
            continue;
        }
        if fs::remove_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

fn is_expired_clipboard_image(path: &Path, now: SystemTime) -> io::Result<bool> {
    let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
        return Ok(false);
    };
    if !file_name.starts_with(CLIPBOARD_IMAGE_FILE_PREFIX)
        || path.extension().and_then(|value| value.to_str()) != Some(PNG_EXTENSION)
    {
        return Ok(false);
    }
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() {
        return Ok(false);
    }
    let Ok(modified) = metadata.modified() else {
        return Ok(false);
    };
    Ok(now
        .duration_since(modified)
        .is_ok_and(|age| age >= TMP_FILE_TTL))
}

struct HostCleanupLock {
    path: PathBuf,
}

impl HostCleanupLock {
    fn try_acquire(path: &Path) -> Option<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok()?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .ok()?;
        file.write_all(std::process::id().to_string().as_bytes())
            .ok()?;
        Some(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for HostCleanupLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::cleanup_agent_tui_tmp_dir;

    #[test]
    fn cleanup_ignores_missing_tmp_dir() {
        let removed =
            cleanup_agent_tui_tmp_dir(std::path::Path::new("target/missing-axon-tui-tmp"))
                .expect("缺失目录应被视为无需清理");

        assert_eq!(removed, 0);
    }
}
