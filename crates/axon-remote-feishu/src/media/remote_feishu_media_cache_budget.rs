//! Feishu 入站媒体临时文件的有界缓存治理。
//!
//! 不维护额外索引：缓存目录本身是唯一状态，写入前按文件大小、数量与年龄回收。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime},
};

use axon_domain::{AxonError, AxonResult};

/// Feishu 临时媒体缓存目录最大字节数。
pub const REMOTE_FEISHU_MEDIA_CACHE_MAX_BYTES: u64 = 64 * 1024 * 1024;
/// Feishu 临时媒体缓存最多保留的文件数。
pub const REMOTE_FEISHU_MEDIA_CACHE_MAX_FILES: usize = 64;
/// Feishu 临时媒体缓存保留时长。
pub const REMOTE_FEISHU_MEDIA_CACHE_RETENTION: Duration = Duration::from_hours(24);

const MEDIA_CACHE_DIR: &str = "remote_feishu_media";
static MEDIA_CACHE_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone)]
struct CachedMediaFile {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
}

#[derive(Debug, Clone, Copy)]
struct MediaCacheBudget {
    max_bytes: u64,
    max_files: usize,
    retention: Duration,
}

const DEFAULT_MEDIA_CACHE_BUDGET: MediaCacheBudget = MediaCacheBudget {
    max_bytes: REMOTE_FEISHU_MEDIA_CACHE_MAX_BYTES,
    max_files: REMOTE_FEISHU_MEDIA_CACHE_MAX_FILES,
    retention: REMOTE_FEISHU_MEDIA_CACHE_RETENTION,
};

/// 在写入媒体文件前回收过期或超额缓存，并在同一临界区落盘。
pub fn write_media_cache_file(cache_root: &Path, target: &Path, body: &[u8]) -> AxonResult<()> {
    let _guard = MEDIA_CACHE_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prepare_media_cache_write(cache_root, target, body.len())?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| AxonError::runtime_failure("无法创建 Feishu 媒体缓存目录"))?;
    }
    fs::write(target, body).map_err(|_| AxonError::runtime_failure("无法写入 Feishu 媒体缓存文件"))
}

/// 在写入媒体文件前回收过期或超额缓存。
fn prepare_media_cache_write(
    cache_root: &Path,
    target: &Path,
    incoming_bytes: usize,
) -> AxonResult<()> {
    prepare_media_cache_write_with_budget(
        cache_root,
        target,
        incoming_bytes,
        DEFAULT_MEDIA_CACHE_BUDGET,
    )
}

fn prepare_media_cache_write_with_budget(
    cache_root: &Path,
    target: &Path,
    incoming_bytes: usize,
    budget: MediaCacheBudget,
) -> AxonResult<()> {
    let incoming_bytes = u64::try_from(incoming_bytes)
        .map_err(|_| AxonError::runtime_failure("Feishu 媒体缓存写入大小无效"))?;
    if incoming_bytes > budget.max_bytes {
        return Err(AxonError::runtime_failure(
            "Feishu 媒体缓存单文件超过容量上限",
        ));
    }

    let cache_dir = cache_root.join(MEDIA_CACHE_DIR);
    fs::create_dir_all(&cache_dir)
        .map_err(|_| AxonError::runtime_failure("无法创建 Feishu 媒体缓存目录"))?;
    let now = SystemTime::now();
    let mut files = media_cache_files(&cache_dir)?;
    files.retain(|file| {
        !is_expired(file, now, budget.retention) || fs::remove_file(&file.path).is_err()
    });
    files.retain(|file| file.path != target);
    files.sort_by_key(|file| file.modified);

    let mut cached_bytes = files.iter().map(|file| file.bytes).sum::<u64>();
    while cached_bytes.saturating_add(incoming_bytes) > budget.max_bytes
        || files.len().saturating_add(1) > budget.max_files
    {
        let Some(file) = files.first() else {
            return Err(AxonError::runtime_failure("Feishu 媒体缓存容量不足"));
        };
        let file = file.clone();
        fs::remove_file(&file.path)
            .map_err(|_| AxonError::runtime_failure("无法回收 Feishu 过期媒体缓存"))?;
        cached_bytes = cached_bytes.saturating_sub(file.bytes);
        files.remove(0);
    }
    Ok(())
}

fn media_cache_files(cache_dir: &Path) -> AxonResult<Vec<CachedMediaFile>> {
    let mut files = Vec::new();
    for kind_dir in fs::read_dir(cache_dir)
        .map_err(|_| AxonError::runtime_failure("无法读取 Feishu 媒体缓存目录"))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
    {
        for entry in fs::read_dir(kind_dir)
            .map_err(|_| AxonError::runtime_failure("无法读取 Feishu 媒体缓存分类目录"))?
            .flatten()
        {
            let path = entry.path();
            let metadata = match entry.metadata() {
                Ok(metadata) if metadata.is_file() => metadata,
                _ => continue,
            };
            files.push(CachedMediaFile {
                path,
                bytes: metadata.len(),
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            });
        }
    }
    Ok(files)
}

fn is_expired(file: &CachedMediaFile, now: SystemTime, retention: Duration) -> bool {
    now.duration_since(file.modified)
        .is_ok_and(|age| age > retention)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_budget_evicts_oldest_files_before_the_new_write() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-feishu-media-cache-budget-{}",
            unique_suffix()
        ));
        let cache_dir = root.join(MEDIA_CACHE_DIR).join("file");
        fs::create_dir_all(&cache_dir).expect("cache dir");
        let first = cache_dir.join("first.bin");
        let second = cache_dir.join("second.bin");
        fs::write(&first, [0_u8; 5]).expect("first");
        std::thread::sleep(Duration::from_millis(10));
        fs::write(&second, [0_u8; 5]).expect("second");

        prepare_media_cache_write_with_budget(
            &root,
            &cache_dir.join("next.bin"),
            5,
            MediaCacheBudget {
                max_bytes: 10,
                max_files: 2,
                retention: Duration::from_mins(1),
            },
        )
        .expect("budget");

        assert!(!first.exists());
        assert!(second.exists());
        let _ = fs::remove_dir_all(root);
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    }
}
