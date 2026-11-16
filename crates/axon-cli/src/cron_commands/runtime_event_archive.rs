//! 会话事件目录的归档与磁盘预算淘汰。
//!
//! 事件已经按会话落在 `<data>/events/<session>/session.vN.jsonl.zstd`，因此清理是
//! 目录级操作：按保留期整目录删除，或在超过磁盘预算时从最旧的会话开始淘汰。
//! 不再有「按行归档成 .jsonl.zst 再删行」这一套：帧本身已经压缩，逐帧再压一次
//! 只会重复占用磁盘。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 事件目录名。
const EVENTS_DIR: &str = "events";
/// 归档目录名。
const ARCHIVE_DIR: &str = "archive";
/// 事件归档子目录名。
const ARCHIVE_SUBDIR: &str = "session-events";
/// 磁盘预算高水位占上限的比例。
const HIGH_WATER_NUMERATOR: u64 = 8;
/// 磁盘预算低水位占上限的比例。
const LOW_WATER_NUMERATOR: u64 = 1;
/// 比例分母。
const WATER_DENOMINATOR: u64 = 2;
/// 数据目录上限：4 GiB。
const MAX_EVENT_DISK_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// 事件归档根：`<data>/archive/session-events`。
pub(super) fn event_archive_root(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(ARCHIVE_DIR)
        .join(ARCHIVE_SUBDIR)
}

/// 事件根：`<data>/events`。
pub(super) fn events_root(database_path: &Path) -> PathBuf {
    database_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(EVENTS_DIR)
}

/// 磁盘预算高水位。
pub(super) fn high_water_bytes() -> u64 {
    MAX_EVENT_DISK_BYTES / WATER_DENOMINATOR * HIGH_WATER_NUMERATOR
}

/// 磁盘预算低水位。
pub(super) fn low_water_bytes() -> u64 {
    MAX_EVENT_DISK_BYTES / WATER_DENOMINATOR * LOW_WATER_NUMERATOR
}

/// 删除最后活动时间早于 `cutoff_ms` 的会话事件目录，返回删除的会话数。
pub(super) fn archive_expired_session_dirs(
    events_root: &Path,
    cutoff_ms: u64,
) -> Result<usize, String> {
    if !events_root.is_dir() {
        return Ok(0);
    }
    let mut removed = 0usize;
    for directory in session_dirs(events_root)? {
        if latest_modified_ms(&directory)? < cutoff_ms {
            fs::remove_dir_all(&directory).map_err(|error| {
                format!("删除过期事件目录失败 {}：{error}", directory.display())
            })?;
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

/// 超过高水位时从最旧会话开始删除，直到落到低水位；返回删除的会话数。
pub(super) fn enforce_events_disk_budget(
    events_root: &Path,
    high_bytes: u64,
    low_bytes: u64,
) -> Result<usize, String> {
    if !events_root.is_dir() {
        return Ok(0);
    }
    let mut sessions = session_dirs_with_size(events_root)?;
    let mut total = sessions
        .iter()
        .fold(0u64, |sum, (_, size, _)| sum.saturating_add(*size));
    if total <= high_bytes {
        return Ok(0);
    }
    sessions.sort_by_key(|(_, _, modified)| *modified);
    let mut removed = 0usize;
    for (directory, size, _) in sessions {
        if total <= low_bytes {
            break;
        }
        fs::remove_dir_all(&directory)
            .map_err(|error| format!("淘汰事件目录失败 {}：{error}", directory.display()))?;
        total = total.saturating_sub(size);
        removed = removed.saturating_add(1);
    }
    Ok(removed)
}

/// 整条会话被删除后，回收它的外置正文归档目录。
pub(super) fn reclaim_session_archive_dir(archive_root: &Path, session_id: &str) {
    if session_id.is_empty()
        || session_id == "."
        || session_id == ".."
        || session_id.contains(['/', '\\'])
    {
        return;
    }
    let directory = archive_root.join(session_id);
    if directory.is_dir() {
        let _ = fs::remove_dir_all(directory);
    }
}

fn session_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(root).map_err(|error| format!("读取事件目录失败：{error}"))?;
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取事件目录条目失败：{error}"))?;
        let path = entry.path();
        if path.is_dir() {
            directories.push(path);
        }
    }
    Ok(directories)
}

fn session_dirs_with_size(root: &Path) -> Result<Vec<(PathBuf, u64, u64)>, String> {
    let mut sessions = Vec::new();
    for directory in session_dirs(root)? {
        let size = directory_size(&directory)?;
        let modified = latest_modified_ms(&directory)?;
        sessions.push((directory, size, modified));
    }
    Ok(sessions)
}

fn directory_size(directory: &Path) -> Result<u64, String> {
    let entries = fs::read_dir(directory).map_err(|error| format!("读取会话目录失败：{error}"))?;
    let mut total = 0u64;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取会话目录条目失败：{error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("读取会话文件元信息失败：{error}"))?;
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

fn latest_modified_ms(directory: &Path) -> Result<u64, String> {
    let entries = fs::read_dir(directory).map_err(|error| format!("读取会话目录失败：{error}"))?;
    let mut latest = 0u64;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取会话目录条目失败：{error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("读取会话文件元信息失败：{error}"))?;
        if !metadata.is_file() {
            continue;
        }
        if let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = SystemTime::now().duration_since(modified)
        {
            let cutoff = SystemTime::now().checked_sub(elapsed).unwrap_or(UNIX_EPOCH);
            if let Ok(since_epoch) = cutoff.duration_since(UNIX_EPOCH) {
                let millis = u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX);
                latest = latest.max(millis);
            }
        }
    }
    Ok(latest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, name: &str, bytes: usize) {
        fs::create_dir_all(dir).expect("建目录");
        fs::write(dir.join(name), vec![b'x'; bytes]).expect("写文件");
    }

    #[test]
    fn expired_session_dirs_are_removed() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        write_file(&events.join("session-old"), "session.v1.jsonl.zstd", 16);
        let removed = archive_expired_session_dirs(&events, u64::MAX).expect("清理");
        assert_eq!(removed, 1);
        assert!(!events.join("session-old").exists());
    }

    #[test]
    fn disk_budget_evicts_oldest_first() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        write_file(&events.join("session-a"), "session.v1.jsonl.zstd", 64);
        write_file(&events.join("session-b"), "session.v1.jsonl.zstd", 64);
        let removed = enforce_events_disk_budget(&events, 1, 1).expect("淘汰");
        assert_eq!(removed, 2, "超过低水位前应持续淘汰");
    }

    #[test]
    fn budget_is_noop_below_high_water() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        write_file(&events.join("session-a"), "session.v1.jsonl.zstd", 8);
        let removed = enforce_events_disk_budget(&events, 1024, 512).expect("检查");
        assert_eq!(removed, 0);
    }
}
