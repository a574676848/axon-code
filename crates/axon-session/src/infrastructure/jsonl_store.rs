//! 按 thread 分片的 JSONL 事件存储。

use std::{
    fmt::Write as _,
    fs,
    io::{Read as _, Seek as _, SeekFrom},
    path::{Path, PathBuf},
};

use crate::{
    application::THREAD_EVENT_SCHEMA_VERSION, domain::ThreadEventRecord,
    ports::ThreadActivationSnapshot, ports::ThreadEventStore,
};
#[cfg(test)]
use axon_jsonl::DEFAULT_SHARD_MAX_BYTES;
use axon_jsonl::ShardedJsonlStore;

/// 保留旧路径编码的最大长度；更长的 thread ID 使用摘要文件名，避免 Windows 路径溢出。
const MAX_LEGACY_THREAD_PREFIX_LENGTH: usize = 96;

/// thread 事件存储目录名。
pub const THREAD_EVENTS_FILE: &str = "threads";
/// 统一 JSONL shard 存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();
/// thread 使用的 shard 大小上限。
#[cfg(test)]
const THREAD_SHARD_MAX_BYTES: u64 = DEFAULT_SHARD_MAX_BYTES;

/// JSONL Thread shard 读取问题。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadEventReadIssue {
    /// shard 文件名。
    pub shard: String,
    /// 严格反序列化错误。
    pub error: String,
}

/// 全量 Thread shard 容错扫描结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadEventScan {
    /// 成功读取的事件。
    pub records: Vec<ThreadEventRecord>,
    /// 无法读取且已隔离的 shard。
    pub issues: Vec<ThreadEventReadIssue>,
}

/// JSONL Thread 事件存储。
#[derive(Debug, Clone, Copy, Default)]
pub struct JsonlThreadEventStore;

/// 反向扫描启动激活状态时的最大物理读取量。
const ACTIVATION_SCAN_BUDGET_BYTES: u64 = 64 * 1024 * 1024;
/// 反向扫描时单次向前读取的字节数。
const ACTIVATION_SCAN_CHUNK_BYTES: usize = 64 * 1024;

impl JsonlThreadEventStore {
    /// 解析 thread 事件存储目录。
    #[must_use]
    pub fn path(root: &Path) -> PathBuf {
        super::session_store_path(root).join(THREAD_EVENTS_FILE)
    }

    /// 删除指定 thread 的分片事件文件；文件不存在时返回 false。
    pub fn remove_thread(&self, root: &Path, thread_id: &str) -> Result<bool, String> {
        SHARD_STORE
            .remove_all(Self::thread_logical_path(root, thread_id))
            .map_err(|_| "无法删除 thread 事件文件".to_owned())
    }

    fn thread_prefix(thread_id: &str) -> String {
        let mut encoded = String::with_capacity(thread_id.len().saturating_mul(2));
        for byte in thread_id.as_bytes() {
            let _ = write!(encoded, "{byte:02x}");
        }
        if encoded.len() <= MAX_LEGACY_THREAD_PREFIX_LENGTH {
            encoded
        } else {
            format!("thread-{}", axon_crypto::sha256_hex(thread_id))
        }
    }

    #[cfg(test)]
    fn shard_path(root: &Path, thread_id: &str, sequence: u64) -> PathBuf {
        ShardedJsonlStore::physical_shard_path(Self::thread_logical_path(root, thread_id), sequence)
    }

    /// 列出指定 thread 的全部分片，按稳定序号排序。
    pub fn thread_shard_paths(root: &Path, thread_id: &str) -> Result<Vec<PathBuf>, String> {
        SHARD_STORE
            .shard_paths(Self::thread_logical_path(root, thread_id))
            .map_err(|_| "无法读取 thread 事件 shard".to_owned())
    }

    /// 用完整记录集原子重写指定 thread 的全部分片；空记录集删除该 thread。
    ///
    /// 该方法只服务于跨 shard 的管理型重写；正常追加仍走 `ThreadEventStore`。
    pub fn rewrite_thread_with_records(
        &self,
        root: &Path,
        thread_id: &str,
        records: &[ThreadEventRecord],
    ) -> Result<(), String> {
        let logical = Self::thread_logical_path(root, thread_id);
        let lines = records
            .iter()
            .map(|record| {
                if record.thread_id != thread_id {
                    return Err("thread 事件重写包含其他 thread 的记录".to_owned());
                }
                serde_json::to_vec(record).map_err(|_| "thread 事件序列化失败".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        SHARD_STORE
            .rewrite_with(logical, |current_lines| {
                let current = Self::parse_lines(&Self::join_lines(current_lines))
                    .map_err(std::io::Error::other)?;
                let expected_sequence = current.last().map_or(0, |record| record.sequence);
                let actual_sequence = records.last().map_or(0, |record| record.sequence);
                if actual_sequence > expected_sequence {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "thread 事件重写序列冲突: expected<={expected_sequence} actual={actual_sequence}"
                        ),
                    ));
                }
                Ok(((), lines.clone()))
            })
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    error.to_string()
                } else {
                    "无法重写 thread 事件存储".to_owned()
                }
            })
    }
    fn thread_logical_path(root: &Path, thread_id: &str) -> PathBuf {
        Self::path(root).join(format!("{}.jsonl", Self::thread_prefix(thread_id)))
    }

    /// 容错扫描全部 shard；单个 shard 损坏不阻断其他只读投影。
    pub fn scan_all(&self, root: &Path) -> Result<ThreadEventScan, String> {
        let mut scan = ThreadEventScan::default();
        for entry in Self::event_files(root)? {
            match Self::read_file(&entry.path()) {
                Ok(records) => scan.records.extend(records),
                Err(error) => scan.issues.push(ThreadEventReadIssue {
                    shard: entry.file_name().to_string_lossy().into_owned(),
                    error,
                }),
            }
        }
        Ok(scan)
    }

    fn event_files(root: &Path) -> Result<Vec<fs::DirEntry>, String> {
        let directory = Self::path(root);
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut entries = fs::read_dir(directory)
            .map_err(|_| "无法读取 thread 事件目录".to_owned())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "无法读取 thread 事件目录条目".to_owned())?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        let mut files = Vec::new();
        for entry in entries {
            if entry
                .file_type()
                .map_err(|_| "thread 事件类型读取失败".to_owned())?
                .is_file()
                && entry.file_name().to_string_lossy().ends_with(".jsonl")
            {
                files.push(entry);
            }
        }
        Ok(files)
    }

    fn read_file(path: &Path) -> Result<Vec<ThreadEventRecord>, String> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(path).map_err(|_| "无法读取 thread 事件".to_owned())?;
        Self::parse_lines(&bytes)
    }

    fn parse_lines(bytes: &[u8]) -> Result<Vec<ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for (index, raw_line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            let raw_line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            if raw_line.is_empty() {
                continue;
            }
            let line = String::from_utf8(raw_line.to_vec())
                .map_err(|_| format!("thread 事件第 {} 行 UTF-8 无效", index + 1))?;
            let trimmed = line.trim();
            let record = serde_json::from_str::<ThreadEventRecord>(trimmed)
                .map_err(|error| format!("thread 事件第 {} 行无效: {error}", index + 1))?;
            if record.schema_version != THREAD_EVENT_SCHEMA_VERSION {
                return Err(format!(
                    "thread 事件第 {} 行 schema 不兼容：expected={} actual={}",
                    index + 1,
                    THREAD_EVENT_SCHEMA_VERSION,
                    record.schema_version
                ));
            }
            records.push(record);
        }
        Ok(records)
    }

    /// 从一个 JSONL 文件尾部反向收集事件；返回 true 表示已找到投影起点。
    /// `None` 表示扫描预算耗尽，调用方必须退回完整读取。
    fn read_activation_tail(
        path: &Path,
        reverse_records: &mut Vec<ThreadEventRecord>,
        scan_budget: &mut u64,
    ) -> Result<Option<bool>, String> {
        if !path.exists() {
            return Ok(Some(false));
        }
        let mut file = fs::File::open(path).map_err(|_| "无法读取 thread 事件".to_owned())?;
        let mut end = file
            .metadata()
            .map_err(|_| "无法读取 thread 事件".to_owned())?
            .len();
        while end > 0 {
            let Some(start) = Self::previous_line_start(&mut file, end, scan_budget)? else {
                return Ok(None);
            };
            if let Some(record) = Self::read_activation_record(&mut file, start, end)? {
                let reached_origin = matches!(
                    record.event,
                    crate::domain::ThreadEvent::ThreadStarted { .. }
                        | crate::domain::ThreadEvent::ThreadCheckpoint { .. }
                );
                reverse_records.push(record);
                if reached_origin {
                    return Ok(Some(true));
                }
            }
            end = start.saturating_sub(1);
        }
        Ok(Some(false))
    }

    fn previous_line_start(
        file: &mut fs::File,
        end: u64,
        scan_budget: &mut u64,
    ) -> Result<Option<u64>, String> {
        let mut scan_end = end;
        while scan_end > 0 {
            let read_start = scan_end.saturating_sub(ACTIVATION_SCAN_CHUNK_BYTES as u64);
            let length = usize::try_from(scan_end - read_start)
                .map_err(|_| "thread 事件扫描窗口过大".to_owned())?;
            let consumed = scan_end - read_start;
            if consumed > *scan_budget {
                return Ok(None);
            }
            file.seek(SeekFrom::Start(read_start))
                .map_err(|_| "无法定位 thread 事件".to_owned())?;
            let mut buffer = vec![0; length];
            file.read_exact(&mut buffer)
                .map_err(|_| "无法读取 thread 事件尾部".to_owned())?;
            if let Some(newline) = buffer.iter().rposition(|byte| *byte == b'\n') {
                let start = read_start + u64::try_from(newline + 1).unwrap_or(u64::MAX);
                let consumed = end - start;
                if consumed > *scan_budget {
                    return Ok(None);
                }
                *scan_budget -= consumed;
                return Ok(Some(start));
            }
            *scan_budget -= consumed;
            if *scan_budget == 0 {
                return Ok(None);
            }
            scan_end = read_start;
        }
        Ok(Some(0))
    }

    fn read_activation_record(
        file: &mut fs::File,
        start: u64,
        end: u64,
    ) -> Result<Option<ThreadEventRecord>, String> {
        if start >= end {
            return Ok(None);
        }
        let length = usize::try_from(end - start).map_err(|_| "thread 事件行过大".to_owned())?;
        file.seek(SeekFrom::Start(start))
            .map_err(|_| "无法定位 thread 事件".to_owned())?;
        let mut line = vec![0; length];
        file.read_exact(&mut line)
            .map_err(|_| "无法读取 thread 事件".to_owned())?;
        let line = String::from_utf8(line).map_err(|_| "thread 事件 UTF-8 无效".to_owned())?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let record = serde_json::from_str::<ThreadEventRecord>(trimmed)
            .map_err(|error| format!("thread 事件无效: {error}"))?;
        if record.schema_version != THREAD_EVENT_SCHEMA_VERSION {
            return Err(format!(
                "thread 事件 schema 不兼容：expected={} actual={}",
                record.schema_version, THREAD_EVENT_SCHEMA_VERSION
            ));
        }
        Ok(Some(record))
    }

    fn join_lines(lines: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for line in lines {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
        bytes
    }
}

impl ThreadEventStore for JsonlThreadEventStore {
    fn append(&self, root: &Path, record: &ThreadEventRecord) -> Result<(), String> {
        let line = serde_json::to_string(record).map_err(|_| "thread 事件序列化失败".to_owned())?;
        SHARD_STORE
            .append_line(
                Self::thread_logical_path(root, &record.thread_id),
                line.as_bytes(),
            )
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    "单条 thread 事件超过 JSONL shard 大小上限".to_owned()
                } else {
                    "无法写入 thread 事件".to_owned()
                }
            })
    }

    fn read_all(&self, root: &Path) -> Result<Vec<ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for entry in Self::event_files(root)? {
            records.extend(Self::read_file(&entry.path())?);
        }
        Ok(records)
    }

    fn replace_thread_with_checkpoint(
        &self,
        root: &Path,
        checkpoint: &ThreadEventRecord,
        expected_sequence: u64,
    ) -> Result<(), String> {
        let logical = Self::thread_logical_path(root, &checkpoint.thread_id);
        SHARD_STORE
            .rewrite_with(logical, |lines| {
                let bytes = Self::join_lines(lines);
                let current = Self::parse_lines(&bytes)
                    .map_err(std::io::Error::other)?;
                let current_sequence = current.last().map_or(0, |record| record.sequence);
                if current_sequence != expected_sequence {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "thread history version conflict: expected={expected_sequence} actual={current_sequence}"
                        ),
                    ));
                }
                let line = serde_json::to_vec(checkpoint).map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                })?;
                Ok(((), vec![line]))
            })
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    error.to_string()
                } else {
                    "无法写入 thread checkpoint".to_owned()
                }
            })
    }

    fn read_thread(&self, root: &Path, thread_id: &str) -> Result<Vec<ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for path in Self::thread_shard_paths(root, thread_id)? {
            records.extend(Self::read_file(&path)?);
        }
        Ok(records)
    }

    fn read_thread_activation(
        &self,
        root: &Path,
        thread_id: &str,
    ) -> Result<ThreadActivationSnapshot, String> {
        let mut paths = Self::thread_shard_paths(root, thread_id)?;
        paths.reverse();
        let mut reverse_records = Vec::new();
        let mut scan_budget = ACTIVATION_SCAN_BUDGET_BYTES;
        for path in &paths {
            match Self::read_activation_tail(path, &mut reverse_records, &mut scan_budget)? {
                Some(true) => {
                    reverse_records.reverse();
                    return Ok(ThreadActivationSnapshot::from_records(&reverse_records));
                }
                Some(false) => {}
                None => break,
            }
        }
        let mut records = Vec::new();
        for path in Self::thread_shard_paths(root, thread_id)? {
            records.extend(Self::read_file(&path)?);
        }
        Ok(ThreadActivationSnapshot::from_records(&records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ThreadService;
    use crate::domain::{
        ThreadEvent, ThreadEventRecord, ThreadSettings, ThreadSource, Turn, TurnStatus,
    };

    fn record(thread_id: &str, sequence: u64) -> ThreadEventRecord {
        ThreadEventRecord {
            schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence,
            recorded_at_ms: sequence,
            event: ThreadEvent::ThreadNamed { name: None },
        }
    }

    fn thread_started(thread_id: &str, sequence: u64) -> ThreadEventRecord {
        ThreadEventRecord {
            schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence,
            recorded_at_ms: sequence,
            event: ThreadEvent::ThreadStarted {
                session_root_id: thread_id.to_owned(),
                parent_thread_id: None,
                forked_from_thread_id: None,
                source: ThreadSource::CliRun,
                ephemeral: false,
                settings: ThreadSettings {
                    cwd: "E:\\fixture".to_owned(),
                    provider: "local".to_owned(),
                    model: "model".to_owned(),
                    selected_skill_name: None,
                },
            },
        }
    }

    fn active_turn(thread_id: &str, sequence: u64) -> ThreadEventRecord {
        ThreadEventRecord {
            schema_version: crate::application::THREAD_EVENT_SCHEMA_VERSION,
            thread_id: thread_id.to_owned(),
            sequence,
            recorded_at_ms: sequence,
            event: ThreadEvent::TurnStarted {
                turn: Turn {
                    turn_id: format!("{thread_id}-active"),
                    sequence: 0,
                    status: TurnStatus::Active,
                    history_items: Vec::new(),
                    token_usage_json: None,
                    started_at_ms: sequence,
                    completed_at_ms: None,
                    interrupted_at_ms: None,
                    failure: None,
                },
            },
        }
    }

    #[test]
    fn activation_reads_latest_events_across_thread_shards() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-activation-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = JsonlThreadEventStore;
        std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
        std::fs::write(
            JsonlThreadEventStore::shard_path(&root, "cross-shard", 0),
            serde_json::to_string(&thread_started("cross-shard", 0)).unwrap() + "\n",
        )
        .unwrap();
        std::fs::write(
            JsonlThreadEventStore::shard_path(&root, "cross-shard", 1),
            serde_json::to_string(&active_turn("cross-shard", 1)).unwrap() + "\n",
        )
        .unwrap();

        let activation = store.read_thread_activation(&root, "cross-shard").unwrap();
        let detail = ThreadService::new(store)
            .read_thread(&root, "cross-shard")
            .unwrap();

        assert_eq!(activation.next_sequence, 2);
        assert_eq!(
            activation.active_turn_id.as_deref(),
            Some("cross-shard-active")
        );
        assert_eq!(detail.turns.len(), 1);
        assert_eq!(detail.turns[0].turn_id, "cross-shard-active");
        assert_eq!(detail.turns[0].status, TurnStatus::Active);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn append_rolls_thread_jsonl_when_shard_exceeds_limit() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-shard-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = JsonlThreadEventStore;
        let first_shard = JsonlThreadEventStore::shard_path(&root, "sharded-thread", 0);
        std::fs::create_dir_all(first_shard.parent().unwrap()).unwrap();
        let mut first_record = record("sharded-thread", 0);
        first_record.event = ThreadEvent::ThreadNamed {
            name: Some(String::new()),
        };
        let baseline_length = serde_json::to_string(&first_record).unwrap().len() as u64;
        let name_length = usize::try_from(THREAD_SHARD_MAX_BYTES - baseline_length - 1).unwrap();
        first_record.event = ThreadEvent::ThreadNamed {
            name: Some("x".repeat(name_length)),
        };
        let first_line = serde_json::to_string(&first_record).unwrap();
        std::fs::write(&first_shard, first_line + "\n").unwrap();
        store.append(&root, &record("sharded-thread", 1)).unwrap();

        let records = store.read_thread(&root, "sharded-thread").unwrap();
        assert_eq!(
            records.iter().map(|item| item.sequence).collect::<Vec<_>>(),
            [0, 1]
        );
        assert!(JsonlThreadEventStore::shard_path(&root, "sharded-thread", 1).exists());

        store
            .replace_thread_with_checkpoint(&root, &record("sharded-thread", 1), 1)
            .unwrap();
        assert!(!JsonlThreadEventStore::shard_path(&root, "sharded-thread", 1).exists());
        assert!(JsonlThreadEventStore::shard_path(&root, "sharded-thread", 0).exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_single_event_exceeding_shard_limit() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-oversize-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = JsonlThreadEventStore;
        let mut event = record("oversize-thread", 0);
        event.event = ThreadEvent::ThreadNamed {
            name: Some("x".repeat(9 * 1024 * 1024)),
        };

        assert!(store.append(&root, &event).is_err());
        assert!(!JsonlThreadEventStore::path(&root).exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rewrite_thread_with_records_replaces_and_clears_all_shards() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-rewrite-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = JsonlThreadEventStore;
        let first = record("rewrite-thread", 1);
        let second = record("rewrite-thread", 2);
        store.append(&root, &first).unwrap();
        store.append(&root, &second).unwrap();
        std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
        std::fs::write(
            JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1),
            format!("{}\n", serde_json::to_string(&second).unwrap()),
        )
        .unwrap();
        assert!(JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1).exists());

        store
            .rewrite_thread_with_records(&root, "rewrite-thread", std::slice::from_ref(&first))
            .unwrap();
        assert_eq!(
            store.read_thread(&root, "rewrite-thread").unwrap(),
            vec![first]
        );
        assert!(!JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1).exists());

        store
            .rewrite_thread_with_records(&root, "rewrite-thread", &[])
            .unwrap();
        assert!(
            store
                .read_thread(&root, "rewrite-thread")
                .unwrap()
                .is_empty()
        );
        assert!(!JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 0).exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn long_thread_ids_use_bounded_stable_paths() {
        let thread_id =
            "task-session-task-1234567890--subagent--root_task-task-session-task-1234567890"
                .repeat(8);
        let first = JsonlThreadEventStore::thread_prefix(&thread_id);
        let second = JsonlThreadEventStore::thread_prefix(&thread_id);

        assert_eq!(first, second);
        assert!(first.starts_with("thread-"));
        assert!(first.len() <= 71);
    }

    #[test]
    fn short_thread_ids_keep_legacy_paths() {
        assert_eq!(
            JsonlThreadEventStore::thread_prefix("short-thread"),
            "73686f72742d746872656164"
        );
    }
}
