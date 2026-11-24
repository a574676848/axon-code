//! Thread 读写遥测：按操作采样耗时与 thread 体量，用于判断长会话的 O(语料) 成本。
//!
//! Thread 事件流以「一行 = 当前全貌」的快照承载压缩投影，因此恢复、收口与换窗都要
//! 解析整份历史。会话越长，单次操作越慢，而这类变慢在日志里没有任何量化读数为证，
//! 事后无法区分「语料本来就这么大」与「路径退化」。
//!
//! 这里按 `resource-metrics` 同一范式补上量化面：每个采样点追加一行 JSONL，按日期分片，
//! 上限 4 MiB，写入失败只告警、绝不影响主流程。三个采样点覆盖每轮恢复、每轮收口与
//! 换窗提交，正好回答「每轮成本是否随会话线性上升」。
//!
//! 落盘位置与 `resource-metrics` / `channel-metrics` 一致，位于 `<axon>/data` 之下：
//! 保留期回收本就按 `data/<样本目录>` 扫描，样本放在运行态根目录之外会让回收扫不到。

use std::path::{Path, PathBuf};
use std::time::Instant;

use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

/// 样本目录名，与 `resource-metrics`、`channel-metrics` 并列。
pub(crate) const THREAD_IO_METRICS_DIR_NAME: &str = "thread-io-metrics";
const THREAD_IO_METRICS_FILE_PREFIX: &str = "samples-";
const THREAD_IO_METRICS_FILE_SUFFIX: &str = ".jsonl";
const THREAD_IO_METRICS_MAX_SHARD_BYTES: u64 = 4 * 1024 * 1024;
const THREAD_IO_METRICS_SHARD_STORE: ShardedJsonlStore =
    ShardedJsonlStore::with_max_shard_bytes(THREAD_IO_METRICS_MAX_SHARD_BYTES);

/// 采样点：每轮恢复、每轮收口、换窗提交。
pub(crate) const OPERATION_PREPARE_RUN: &str = "prepareRun";
/// 收口本轮到 thread。
pub(crate) const OPERATION_MIRROR_TERMINAL: &str = "mirrorTerminal";
/// 换窗安装活动窗口投影。
pub(crate) const OPERATION_COMPACT_THREAD: &str = "compactThread";

/// 一次 thread 读写采样。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ThreadIoSample {
    /// 采样时刻（毫秒时间戳）。
    pub sampled_at_ms: u64,
    /// 进程标识。
    pub pid: u32,
    /// 采样点名称，取值见本模块的 `OPERATION_*`。
    pub operation: String,
    /// 这次操作是否成功。
    pub ok: bool,
    /// 从开始到采样写下的耗时（毫秒）。
    pub duration_ms: u64,
    /// 该 thread 当前的分片数。
    pub shard_count: u32,
    /// 该 thread 当前分片总字节数。
    pub thread_bytes: u64,
    /// 被操作的 thread 标识。
    pub thread_id: String,
}

/// 记录一次 thread 操作；`started` 由调用方在操作开始时取得。
///
/// 采样永远不影响主流程：目录创建、序列化或追加失败都只留一条告警，
/// 调用方继续按原有结果返回。
pub(crate) fn record(
    axon_root: &Path,
    operation: &str,
    thread_id: &str,
    started: Instant,
    ok: bool,
) {
    let (shard_count, thread_bytes) = thread_size(axon_root, thread_id);
    let sample = ThreadIoSample {
        sampled_at_ms: current_ms(),
        pid: std::process::id(),
        operation: operation.to_owned(),
        ok,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        shard_count,
        thread_bytes,
        thread_id: thread_id.to_owned(),
    };
    if let Err(error) = append_sample(axon_root, &sample) {
        axon_logger::warning(&format!("thread 读写遥测落盘失败：{error}"));
    }
}

/// 统计 thread 的分片数与总字节；读不出来时按零处理。
///
/// 只做目录枚举与元数据读取，不解析任何事件内容，因此可以放在采样路径上。
fn thread_size(axon_root: &Path, thread_id: &str) -> (u32, u64) {
    let Ok(paths) = axon_session::JsonlThreadEventStore::thread_shard_paths(axon_root, thread_id)
    else {
        return (0, 0);
    };
    let bytes = paths.iter().fold(0_u64, |total, path| {
        total.saturating_add(std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
    });
    (u32::try_from(paths.len()).unwrap_or(u32::MAX), bytes)
}

/// 返回 thread 读写遥测的分片目录，供写出与回收共用同一口径。
pub(crate) fn metrics_directory(axon_root: &Path) -> PathBuf {
    axon_root
        .join(axon_config::AXON_RUNTIME_DATA_DIR)
        .join(THREAD_IO_METRICS_DIR_NAME)
}

fn append_sample(axon_root: &Path, sample: &ThreadIoSample) -> Result<(), String> {
    let directory = metrics_directory(axon_root);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("创建 thread 遥测目录失败：{error}"))?;
    let line = serde_json::to_string(sample).map_err(|error| format!("序列化采样失败：{error}"))?;
    let date = sample_date(sample.sampled_at_ms);
    THREAD_IO_METRICS_SHARD_STORE
        .append_line(logical_path(&directory, &date), line.as_bytes())
        .map_err(|error| format!("追加采样失败：{error}"))
}

fn logical_path(directory: &Path, date: &str) -> PathBuf {
    directory.join(format!(
        "{THREAD_IO_METRICS_FILE_PREFIX}{date}{THREAD_IO_METRICS_FILE_SUFFIX}"
    ))
}

fn sample_date(timestamp_ms: u64) -> String {
    let timestamp_ms = i64::try_from(timestamp_ms).unwrap_or(i64::MAX);
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn current_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 样本按 camelCase 落盘并能原样回读，供事后按时间轴分析。
    #[test]
    fn samples_round_trip_through_json() {
        let sample = ThreadIoSample {
            sampled_at_ms: 1_700_000_000_000,
            pid: 42,
            operation: OPERATION_COMPACT_THREAD.to_owned(),
            ok: true,
            duration_ms: 1_234,
            shard_count: 8,
            thread_bytes: 60 * 1024 * 1024,
            thread_id: "thread-1".to_owned(),
        };
        let line = serde_json::to_string(&sample).expect("序列化");
        assert!(
            line.contains("\"durationMs\"") && line.contains("\"threadBytes\""),
            "落盘字段为 camelCase：{line}"
        );
        let decoded: ThreadIoSample = serde_json::from_str(&line).expect("回读");
        assert_eq!(decoded, sample);
    }

    /// 落盘路径按日期分片，避免单文件无限增长。
    #[test]
    fn samples_are_sharded_by_date() {
        let directory = PathBuf::from("data");
        assert_eq!(
            logical_path(&directory, "2026-11-20"),
            PathBuf::from("data").join("samples-2026-11-20.jsonl")
        );
    }

    /// 样本目录必须落在运行态 `data` 之下，与保留期回收同口径。
    #[test]
    fn sample_directory_lives_under_runtime_data() {
        let root = PathBuf::from("axon-root");
        assert_eq!(
            metrics_directory(&root),
            root.join(axon_config::AXON_RUNTIME_DATA_DIR)
                .join(THREAD_IO_METRICS_DIR_NAME)
        );
    }

    /// 读不到 thread 分片时按零统计，采样本身不得失败。
    #[test]
    fn missing_thread_reports_zero_size() {
        let root = axon_test_support::test_temp_root().join("thread_io_metrics_missing");
        let (shards, bytes) = thread_size(&root, "no-such-thread");
        assert_eq!((shards, bytes), (0, 0));
        let _ = std::fs::remove_dir_all(root);
    }
}
