use std::{
    collections::BTreeSet,
    fs::{self},
    path::{Path, PathBuf},
};

use axon_jsonl::ShardedJsonlStore;

const CHANNEL_METRICS_DIR: &str = "channel-metrics";
const CHANNEL_METRICS_FILE_PREFIX: &str = "samples-";
const CHANNEL_METRICS_FILE_SUFFIX: &str = ".jsonl";
const MILLISECONDS_PER_SECOND: u64 = 1_000;
const SECONDS_PER_MINUTE: u64 = 60;
const MINUTES_PER_HOUR: u64 = 60;
const HOURS_PER_DAY: u64 = 24;
/// Cron 清理与写入侧使用同一 shard 合同。
const CHANNEL_METRICS_SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

pub(super) fn retention_ms() -> u64 {
    super::SESSION_RETENTION_DAYS
        .saturating_mul(HOURS_PER_DAY)
        .saturating_mul(MINUTES_PER_HOUR)
        .saturating_mul(SECONDS_PER_MINUTE)
        .saturating_mul(MILLISECONDS_PER_SECOND)
}

pub(super) fn dry_run_channel_metrics_count(root: &Path, cutoff_ms: u64) -> Result<usize, String> {
    let directory = root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(CHANNEL_METRICS_DIR);
    if !directory.is_dir() {
        return Ok(0);
    }
    let mut total = 0_usize;
    for logical_path in channel_metrics_logical_paths(&directory)? {
        for line in CHANNEL_METRICS_SHARD_STORE
            .read_lines(&logical_path)
            .map_err(|_| "读取渠道指标文件失败".to_owned())?
        {
            let value =
                serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(&line)).ok();
            if value
                .and_then(|value| {
                    value
                        .get("timestamp_ms")
                        .and_then(serde_json::Value::as_u64)
                })
                .is_some_and(|timestamp| timestamp < cutoff_ms)
            {
                total = total.saturating_add(1);
            }
        }
    }
    Ok(total)
}

pub(super) fn cleanup_channel_metrics(root: &Path, cutoff_ms: u64) -> Result<usize, String> {
    let directory = root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(CHANNEL_METRICS_DIR);
    let mut paths = channel_metrics_logical_paths(&directory)?
        .into_iter()
        .collect::<Vec<_>>();
    paths.sort();
    paths.into_iter().try_fold(0_usize, |deleted, path| {
        cleanup_channel_metrics_file(&path, cutoff_ms).map(|count| deleted + count)
    })
}

fn channel_metrics_logical_paths(directory: &Path) -> Result<BTreeSet<PathBuf>, String> {
    if !directory.is_dir() {
        return Ok(BTreeSet::new());
    }
    let mut logical_paths = BTreeSet::new();
    for entry in fs::read_dir(directory).map_err(|_| "读取渠道指标目录失败".to_owned())? {
        let path = entry
            .map_err(|_| "读取渠道指标目录项失败".to_owned())?
            .path();
        if let Some(logical_path) = channel_metrics_logical_path(&path) {
            logical_paths.insert(logical_path);
        }
    }
    Ok(logical_paths)
}

fn channel_metrics_logical_path(shard_path: &Path) -> Option<PathBuf> {
    let name = shard_path.file_name()?.to_str()?;
    let stem = name.strip_suffix(CHANNEL_METRICS_FILE_SUFFIX)?;
    let (date, shard) = stem.split_once('.')?;
    if shard.parse::<u64>().is_err() || shard.len() != 6 {
        return None;
    }
    Some(shard_path.parent()?.join(format!(
        "{CHANNEL_METRICS_FILE_PREFIX}{date}{CHANNEL_METRICS_FILE_SUFFIX}"
    )))
}

fn cleanup_channel_metrics_file(path: &Path, cutoff_ms: u64) -> Result<usize, String> {
    CHANNEL_METRICS_SHARD_STORE
        .rewrite_with(path, |lines| {
            let retained: Vec<_> = lines
                .iter()
                .filter(|line| {
                    let timestamp_ms =
                        serde_json::from_str::<serde_json::Value>(&String::from_utf8_lossy(line))
                            .ok()
                            .and_then(|value| {
                                value
                                    .get("timestamp_ms")
                                    .and_then(serde_json::Value::as_u64)
                            });
                    timestamp_ms.is_none_or(|timestamp| timestamp >= cutoff_ms)
                })
                .cloned()
                .collect();
            let deleted = lines.len().saturating_sub(retained.len());
            Ok((deleted, retained))
        })
        .map_err(|error| format!("清理渠道指标文件失败：{error}"))
}
