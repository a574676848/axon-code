//! 按 thread 分片的 JSONL 事件存储。

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use crate::{
    application::THREAD_EVENT_SCHEMA_VERSION, domain::ThreadEventRecord,
    ports::ThreadActivationSnapshot, ports::ThreadEventStore,
};
use axon_jsonl::ShardedJsonlStore;
#[cfg(test)]
use axon_jsonl::{DEFAULT_SHARD_MAX_BYTES, MAX_SINGLE_RECORD_BYTES};

/// 保留旧路径编码的最大长度；更长的 thread ID 使用摘要文件名，避免 Windows 路径溢出。
const MAX_LEGACY_THREAD_PREFIX_LENGTH: usize = 96;

/// thread 事件存储目录名。
pub const THREAD_EVENTS_FILE: &str = "threads";
/// 统一 JSONL shard 存储。
pub(crate) const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();
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

const ACTIVATION_SCAN_BUDGET_BYTES: u64 = 64 * 1024 * 1024;

impl JsonlThreadEventStore {
    /// 解析 thread 事件存储目录。
    #[must_use]
    pub fn path(root: &Path) -> PathBuf {
        super::session_store_path(root).join(THREAD_EVENTS_FILE)
    }

    /// 删除指定 thread 的分片事件文件；文件不存在时返回 false。
    pub fn remove_thread(&self, root: &Path, thread_id: &str) -> Result<bool, String> {
        let removed = SHARD_STORE
            .remove_all(Self::thread_logical_path(root, thread_id))
            .map_err(|_| "无法删除 thread 事件文件".to_owned())?;
        if removed {
            // 同步删除索引条目，否则名册与保留判定会把已删会话当成幽灵继续返回。
            super::thread_index::append_thread_removed(root, thread_id);
        }
        Ok(removed)
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
    ///
    /// `expected_sequence` 是调用方读到快照时该 thread 的末条事件序号。读-改-写必须
    /// 带上这一条校验：两次读之间别人追加的事件一旦被 `lines.clone()` 整段覆盖，就是
    /// 一次静默的丢失更新。序号对不上时返回冲突，调用方下一轮重来。
    pub fn rewrite_thread_with_records(
        &self,
        root: &Path,
        thread_id: &str,
        expected_sequence: u64,
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
                let actual_sequence = current.last().map_or(0, |record| record.sequence);
                if actual_sequence != expected_sequence {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "thread 事件重写冲突: expected={expected_sequence} actual={actual_sequence}"
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

    /// 受锁截断撕裂的尾行；调用方随后重读一次。
    pub(crate) fn repair_torn_tail(root: &Path, thread_id: &str) -> Result<(), String> {
        SHARD_STORE
            .repair_torn_tail(Self::thread_logical_path(root, thread_id))
            .map_err(|_| "无法修复 thread 撕裂尾行".to_owned())
    }

    /// 这条 thread 是否留下了未完成重写的临时文件。
    ///
    /// 有临时文件却一条事件都读不到，说明重写崩在了替换分片的中间：读侧必须报错，
    /// 不能把它当成「从不存在」的新会话重建，否则旧历史会被孤儿文件永久埋掉。
    pub(crate) fn thread_has_unfinished_write(root: &Path, thread_id: &str) -> bool {
        SHARD_STORE
            .has_stale_rewrite_temporaries(Self::thread_logical_path(root, thread_id))
            .unwrap_or(false)
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
    fn has_unfinished_write(&self, root: &Path, thread_id: &str) -> bool {
        Self::thread_has_unfinished_write(root, thread_id)
    }

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
            })?;
        super::thread_index::append_index_patch(root, record);
        Ok(())
    }

    fn append_at_expected_sequence(
        &self,
        root: &Path,
        record: &ThreadEventRecord,
        expected_sequence: u64,
    ) -> Result<(), String> {
        let logical = Self::thread_logical_path(root, &record.thread_id);
        let line = serde_json::to_vec(record).map_err(|_| "thread 事件序列化失败".to_owned())?;
        SHARD_STORE
            .append_with(logical, |lines| {
                let current =
                    Self::parse_lines(&Self::join_lines(lines)).map_err(std::io::Error::other)?;
                let actual = current.last().map_or(0, |event| event.sequence);
                if actual != expected_sequence {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "thread event sequence conflict: expected={expected_sequence} actual={actual}"
                        ),
                    ));
                }
                Ok(((), vec![line.clone()]))
            })
            .map_err(|error| error.to_string())?;
        super::thread_index::append_index_patch(root, record);
        Ok(())
    }

    fn read_thread(&self, root: &Path, thread_id: &str) -> Result<Vec<ThreadEventRecord>, String> {
        // 只读本 thread 的分片：trait 默认实现会 read_all（整语料），恢复与会话命令
        // 一旦走到那条路径，单次读取就退化成 O(全部会话)。
        Self::read_thread_full(root, thread_id)
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

    fn read_thread_activation(
        &self,
        root: &Path,
        thread_id: &str,
    ) -> Result<ThreadActivationSnapshot, String> {
        // 尾行撕裂时先受锁截断再重扫一次：激活快照是「能不能继续这一轮」的唯一判据，
        // 读失败会让会话既不能恢复也不能继续。
        match self.scan_thread_activation(root, thread_id) {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => {
                Self::repair_torn_tail(root, thread_id)?;
                self.scan_thread_activation(root, thread_id)
                    .map_err(|_| error)
            }
        }
    }
}

impl JsonlThreadEventStore {
    /// 激活快照的一次扫描：不修复，撕裂尾行由调用方截断后重试。
    fn scan_thread_activation(
        &self,
        root: &Path,
        thread_id: &str,
    ) -> Result<ThreadActivationSnapshot, String> {
        // 从最后一个分片整体回读（每片 ≤ 8 MiB），不再「每行读一次 64 KiB 窗口」：
        // 后者在长会话上把物理读放大到 S × (64 KiB / 平均行长)。读到「没有 active turn」
        // 的状态即停止；预算耗尽退回完整读，不猜。
        let paths = Self::thread_shard_paths(root, thread_id)?;
        if paths.is_empty() {
            return Ok(ThreadActivationSnapshot::default());
        }
        let mut budget = ACTIVATION_SCAN_BUDGET_BYTES;
        let mut scanned: Vec<ThreadEventRecord> = Vec::new();
        let mut resolved = false;
        for path in paths.iter().rev() {
            let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
            if size > budget {
                break;
            }
            budget -= size;
            let mut records = Self::read_file(path)?;
            records.reverse();
            scanned.append(&mut records);
            if super::thread_tail::activation_settled(&scanned) {
                resolved = true;
                break;
            }
        }
        if resolved {
            scanned.reverse();
            return Ok(ThreadActivationSnapshot::from_records(&scanned));
        }
        Ok(ThreadActivationSnapshot::from_records(
            &Self::read_thread_full(root, thread_id)?,
        ))
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

    /// 只剩重写临时文件时不得当作「新会话」：这是崩溃现场，必须报错而不是重建。
    #[test]
    fn thread_with_only_rewrite_debris_is_not_treated_as_missing() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-debris-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        std::fs::create_dir_all(JsonlThreadEventStore::path(&root)).unwrap();
        // 落盘文件名是编码后的 thread 前缀，这里从分片名反推同一前缀，保证临时文件命名口径一致。
        let shard = JsonlThreadEventStore::shard_path(&root, "debris", 0);
        let file_name = shard.file_name().unwrap().to_string_lossy().into_owned();
        let prefix = file_name.split('.').next().unwrap().to_owned();
        let temporary = shard.with_file_name(format!("{prefix}.rewrite0.tmp"));
        std::fs::write(&temporary, b"{}").unwrap();
        assert!(
            JsonlThreadEventStore::thread_has_unfinished_write(&root, "debris"),
            "临时文件必须被识别为未完成写入（{}）",
            temporary.display()
        );

        let service = ThreadService::new(JsonlThreadEventStore);
        let error = service
            .read_thread_optional(&root, "debris")
            .expect_err("有未完成写入痕迹时必须报错");
        assert!(error.contains("不完整"), "{error}");
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
    fn oversize_event_occupies_its_own_shard() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-oversize-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = JsonlThreadEventStore;
        store.append(&root, &record("oversize-thread", 0)).unwrap();
        assert!(JsonlThreadEventStore::shard_path(&root, "oversize-thread", 0).exists());

        let mut oversize = record("oversize-thread", 1);
        oversize.event = ThreadEvent::ThreadNamed {
            name: Some("x".repeat(usize::try_from(THREAD_SHARD_MAX_BYTES).unwrap() + 1_024)),
        };
        store
            .append(&root, &oversize)
            .expect("超过分片预算的单条事件应独占新分片，而不是被拒绝");

        assert!(
            JsonlThreadEventStore::shard_path(&root, "oversize-thread", 1).exists(),
            "超预算事件必须落进下一个分片"
        );
        assert_eq!(
            store.read_thread(&root, "oversize-thread").unwrap().len(),
            2,
            "两条记录都必须保留"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_single_event_beyond_hard_record_limit() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-thread-hard-limit-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let store = JsonlThreadEventStore;
        let mut event = record("hard-limit-thread", 0);
        event.event = ThreadEvent::ThreadNamed {
            name: Some("x".repeat(usize::try_from(MAX_SINGLE_RECORD_BYTES).unwrap() + 1)),
        };

        assert!(
            store.append(&root, &event).is_err(),
            "超过单条记录硬上限必须拒绝写入"
        );
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
            .rewrite_thread_with_records(&root, "rewrite-thread", 2, std::slice::from_ref(&first))
            .unwrap();
        assert_eq!(
            store.read_thread(&root, "rewrite-thread").unwrap(),
            vec![first]
        );
        assert!(!JsonlThreadEventStore::shard_path(&root, "rewrite-thread", 1).exists());

        store
            .rewrite_thread_with_records(&root, "rewrite-thread", 1, &[])
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
