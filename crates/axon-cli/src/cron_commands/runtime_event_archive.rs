//! 会话事件目录的归档与磁盘预算淘汰。
//!
//! 事件已经按会话落在 `<data>/events/<session>/session.vN.jsonl.zstd`，因此清理是
//! 目录级操作：按保留期整目录删除，或在超过磁盘预算时从最旧的会话开始淘汰。
//! 不再有「按行归档成 .jsonl.zst 再删行」这一套：帧本身已经压缩，逐帧再压一次
//! 只会重复占用磁盘。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// 事件目录名。
const EVENTS_DIR: &str = "events";
/// 归档目录名。
const ARCHIVE_DIR: &str = "archive";
/// 事件归档子目录名。
const ARCHIVE_SUBDIR: &str = "session-events";
/// 磁盘预算高水位占上限的百分比。
const HIGH_WATER_PERCENT: u64 = 80;
/// 磁盘预算低水位占上限的百分比。
const LOW_WATER_PERCENT: u64 = 20;
/// 比例基数。
const PERCENT_BASE: u64 = 100;
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

/// 磁盘预算高水位：上限的 80%。
pub(super) fn high_water_bytes() -> u64 {
    MAX_EVENT_DISK_BYTES / PERCENT_BASE * HIGH_WATER_PERCENT
}

/// 磁盘预算低水位：上限的 20%。
pub(super) fn low_water_bytes() -> u64 {
    MAX_EVENT_DISK_BYTES / PERCENT_BASE * LOW_WATER_PERCENT
}

/// 删除最后活动时间早于 `cutoff_ms` 的会话事件目录，返回删除的会话数。
///
/// 正文归档目录与事件目录同名（同一个编码片段），因此事件目录过期时归档一起删：只删
/// 事件目录会把归档留成孤儿，而归档既不适用 3 天文件保留期、也不在会话族的删除清单里。
pub(super) fn archive_expired_session_dirs(
    events_root: &Path,
    archive_root: &Path,
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
            if let Some(name) = directory.file_name() {
                let archived = archive_root.join(name);
                if archived.is_dir() {
                    fs::remove_dir_all(&archived).map_err(|error| {
                        format!("删除正文归档目录失败 {}：{error}", archived.display())
                    })?;
                }
            }
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

/// 超过高水位时从最旧会话开始删除，直到落到低水位；返回删除的会话数。
///
/// 每个会话的占用按「事件目录 + 同名正文归档目录」合计：归档是同一份正文的另一份
/// 副本，只统计事件目录会让预算在看不清的地方被吃掉。
///
/// `protected` 是仍在推进的会话目录名（已按事件目录规则编码）：它们的事件还会被
/// 读写，磁盘紧张也不能删。全部候选都被保护时本函数如实返回 0，由上层报警，
/// 不做「删活跃会话换水位」的交换。
pub(super) fn enforce_events_disk_budget(
    events_root: &Path,
    archive_root: &Path,
    high_bytes: u64,
    low_bytes: u64,
    protected: &HashSet<String>,
) -> Result<usize, String> {
    if !events_root.is_dir() {
        return Ok(0);
    }
    let mut sessions = session_dirs_with_size(events_root, archive_root)?;
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
        let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if protected.contains(name) {
            continue;
        }
        fs::remove_dir_all(&directory)
            .map_err(|error| format!("淘汰事件目录失败 {}：{error}", directory.display()))?;
        // 同名正文归档一起淘汰：它是这条会话正文的另一份副本，留下就成了孤儿。
        let archived = archive_root.join(name);
        if archived.is_dir() {
            fs::remove_dir_all(&archived)
                .map_err(|error| format!("淘汰正文归档目录失败 {}：{error}", archived.display()))?;
        }
        total = total.saturating_sub(size);
        removed = removed.saturating_add(1);
    }
    Ok(removed)
}

/// 整条会话被删除后，回收它的外置正文归档目录。
///
/// 目录名与事件目录同一套编码（`EventLog::encode_segment`），因此这里必须先编码：
/// 直接拿原始会话标识拼路径，遇到非 ULID 标识（含 `:`、非 ASCII）会永远对不上，
/// 归档目录就此变成没人回收的孤儿。
pub(super) fn reclaim_session_archive_dir(archive_root: &Path, session_id: &str) {
    if session_id.is_empty() || session_id == "." || session_id == ".." {
        return;
    }
    let directory = archive_root.join(axon_eventlog::EventLog::encode_segment(session_id));
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

fn session_dirs_with_size(
    root: &Path,
    archive_root: &Path,
) -> Result<Vec<(PathBuf, u64, u64)>, String> {
    let mut sessions = Vec::new();
    for directory in session_dirs(root)? {
        let mut size = directory_size(&directory)?;
        // 同名正文归档计入这条会话的占用：预算必须看见磁盘上的全部副本。
        if let Some(name) = directory.file_name() {
            let archived = archive_root.join(name);
            if archived.is_dir() {
                size = size.saturating_add(directory_size(&archived)?);
            }
        }
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
        let removed = archive_expired_session_dirs(&events, &root.path().join("archive"), u64::MAX)
            .expect("清理");
        assert_eq!(removed, 1);
        assert!(!events.join("session-old").exists());
    }

    #[test]
    fn disk_budget_evicts_oldest_first() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        write_file(&events.join("session-a"), "session.v1.jsonl.zstd", 64);
        write_file(&events.join("session-b"), "session.v1.jsonl.zstd", 64);
        let removed = enforce_events_disk_budget(
            &events,
            &root.path().join("archive"),
            1,
            1,
            &HashSet::new(),
        )
        .expect("淘汰");
        assert_eq!(removed, 2, "超过低水位前应持续淘汰");
    }

    #[test]
    fn budget_is_noop_below_high_water() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        write_file(&events.join("session-a"), "session.v1.jsonl.zstd", 8);
        let removed = enforce_events_disk_budget(
            &events,
            &root.path().join("archive"),
            1024,
            512,
            &HashSet::new(),
        )
        .expect("检查");
        assert_eq!(removed, 0);
    }

    #[test]
    fn budget_never_evicts_active_session_dirs() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        write_file(&events.join("session-live"), "session.v1.jsonl.zstd", 64);
        write_file(&events.join("session-dead"), "session.v1.jsonl.zstd", 64);
        let protected = HashSet::from(["session-live".to_owned()]);
        // 低水位 1 字节：不保护就得全删；保护后活跃目录必须留下。
        let removed =
            enforce_events_disk_budget(&events, &root.path().join("archive"), 1, 1, &protected)
                .expect("淘汰");
        assert_eq!(removed, 1, "只淘汰非活跃会话");
        assert!(
            events.join("session-live").exists(),
            "活跃会话事件目录不得被淘汰"
        );
        assert!(!events.join("session-dead").exists());
    }

    #[test]
    fn expired_session_dirs_take_their_body_archive_with_them() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        let archive = root.path().join("archive");
        write_file(&events.join("session-old"), "session.v1.jsonl.zstd", 16);
        write_file(&archive.join("session-old"), "body.jsonl.zst", 16);
        let removed = archive_expired_session_dirs(&events, &archive, u64::MAX).expect("清理");
        assert_eq!(removed, 1);
        assert!(!events.join("session-old").exists());
        assert!(
            !archive.join("session-old").exists(),
            "正文归档必须跟着事件目录一起删，否则会变成没人回收的孤儿"
        );
    }

    #[test]
    fn disk_budget_counts_and_evicts_the_body_archive() {
        let root = tempfile::tempdir().expect("临时目录");
        let events = root.path().join("events");
        let archive = root.path().join("archive");
        write_file(&events.join("session-old"), "session.v1.jsonl.zstd", 8);
        // 归档比事件目录还大：只统计事件目录会让预算看不见它。
        write_file(&archive.join("session-old"), "body.jsonl.zst", 64);
        write_file(&events.join("session-new"), "session.v1.jsonl.zstd", 8);
        let removed =
            enforce_events_disk_budget(&events, &archive, 1, 1, &HashSet::new()).expect("淘汰");
        assert_eq!(removed, 2, "两个会话都会被淘汰到低水位");
        assert!(!archive.join("session-old").exists());
    }

    #[test]
    fn water_marks_stay_inside_the_disk_cap() {
        assert_eq!(high_water_bytes(), MAX_EVENT_DISK_BYTES / 100 * 80);
        assert_eq!(low_water_bytes(), MAX_EVENT_DISK_BYTES / 100 * 20);
        assert!(
            high_water_bytes() < MAX_EVENT_DISK_BYTES,
            "高水位必须低于上限，否则预算永远不触发"
        );
        assert!(low_water_bytes() < high_water_bytes());
    }
}
