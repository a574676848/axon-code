//! 用户级运行态 store 路径与事实 JSONL 分片 retention 约定。

use std::{io, path::Path, path::PathBuf};

use axon_config::{USER_RUNTIME_DATA_DIR, USER_RUNTIME_SUBDIR};
use axon_jsonl::ShardedJsonlStore;
use serde_json::Value;

/// 运行态事实记录使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// 解析用户级运行态文件路径：`<axon-root>/data/runtime/<file>`。
#[must_use]
pub(crate) fn runtime_store_file_path(root: impl AsRef<Path>, file_name: &str) -> PathBuf {
    root.as_ref()
        .join(USER_RUNTIME_DATA_DIR)
        .join(USER_RUNTIME_SUBDIR)
        .join(file_name)
}

/// 在同一锁区间内追加记录并执行时间保留策略。
pub(crate) fn append_retained_jsonl(
    path: &Path,
    line: &str,
    recorded_at_ms: u64,
    max_age_ms: u64,
) -> std::io::Result<()> {
    SHARD_STORE.append_retained_line(path, line, recorded_at_ms, max_age_ms, json_recorded_at_ms)
}

fn json_recorded_at_ms(line: &[u8]) -> std::io::Result<u64> {
    let value: Value = serde_json::from_slice(line)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    value
        .get("recordedAtMs")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "JSONL 记录缺少 recordedAtMs"))
}
