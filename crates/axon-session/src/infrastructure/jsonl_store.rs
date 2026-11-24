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
    /// 单条记录的唯一编码入口。
    ///
    /// 超过内联阈值的记录把正文外置，日志行只留引用：这是「单条记录有界」从口号变成
    /// 事实的地方。写入方（追加、序号校验追加、checkpoint 重写、管理型重写）都走这里，
    /// 避免任何一条路径把整份正文重新塞回日志。
    fn encode_record_line(root: &Path, record: &ThreadEventRecord) -> Result<Vec<u8>, String> {
        let inline = serde_json::to_vec(record).map_err(|_| "thread 事件序列化失败".to_owned())?;
        if !super::thread_event_body::exceeds_inline_limit(inline.len()) {
            return Ok(inline);
        }
        let reference = super::thread_event_body::externalize(
            root,
            &record.thread_id,
            record.sequence,
            &record.event,
        )?;
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": record.schema_version,
            "threadId": record.thread_id,
            "sequence": record.sequence,
            "recordedAtMs": record.recorded_at_ms,
            "event": reference.to_json(),
        }))
        .map_err(|_| "thread 事件序列化失败".to_owned())
    }

    /// 单条记录的唯一解码入口：内联记录直接解析，引用记录还原正文后再还原事件。
    ///
    /// 先按完整记录解析：绝大多数记录是内联的，这条快路径不为兼容付出成本；只有
    /// 解析失败且 `event.type` 是正文引用时，才按引用去读正文。正文缺失或摘要不符
    /// 的错误必须原样上抛，不能被「行无效」掩盖。
    pub(crate) fn decode_record_line(
        root: &Path,
        raw_line: &[u8],
    ) -> Result<ThreadEventRecord, String> {
        match serde_json::from_slice::<ThreadEventRecord>(raw_line) {
            Ok(record) => Ok(record),
            Err(error) => Self::decode_external_record(root, raw_line).map_err(|external| {
                if external.is_empty() {
                    format!("thread 事件行无效：{error}")
                } else {
                    external
                }
            }),
        }
    }

    /// 引用记录的解码；不是引用形态时返回空错误让调用方保留原始解析错误。
    fn decode_external_record(root: &Path, raw_line: &[u8]) -> Result<ThreadEventRecord, String> {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(raw_line) else {
            return Err(String::new());
        };
        let Some(reference) = value
            .get("event")
            .and_then(super::thread_event_body::ThreadEventBodyRef::from_json)
        else {
            return Err(String::new());
        };
        let schema_version = value
            .get("schemaVersion")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| "thread 正文引用缺少 schema 版本".to_owned())?;
        let thread_id = value
            .get("threadId")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "thread 正文引用缺少 thread 标识".to_owned())?
            .to_owned();
        let sequence = value
            .get("sequence")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "thread 正文引用缺少事件序号".to_owned())?;
        let recorded_at_ms = value
            .get("recordedAtMs")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "thread 正文引用缺少记录时间".to_owned())?;
        let event = super::thread_event_body::resolve(root, &thread_id, sequence, &reference)?;
        Ok(ThreadEventRecord {
            schema_version,
            thread_id,
            sequence,
            recorded_at_ms,
            event,
        })
    }

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
        // 正文与分片同生命周期：分片没了而正文还在，只会留下永远读不到的副本。
        super::thread_event_body::remove_bodies(root, thread_id)?;
        if removed {
            // 同步删除索引条目，否则名册与保留判定会把已删会话当成幽灵继续返回。
            super::thread_index::append_thread_removed(root, thread_id);
        }
        Ok(removed)
    }

    pub(crate) fn thread_prefix(thread_id: &str) -> String {
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
                Self::encode_record_line(root, record)
            })
            .collect::<Result<Vec<_>, _>>()?;
        SHARD_STORE
            .rewrite_with(logical, |current_lines| {
                let current = Self::parse_lines(root, &Self::join_lines(current_lines))
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
            })?;
        // 重写之后旧事件的正文失去引用；只保留仍在新记录集里的正文。
        let kept = records
            .iter()
            .map(|record| record.sequence)
            .collect::<Vec<_>>();
        super::thread_event_body::prune_bodies_to(root, thread_id, &kept)
    }
    fn thread_logical_path(root: &Path, thread_id: &str) -> PathBuf {
        Self::path(root).join(format!("{}.jsonl", Self::thread_prefix(thread_id)))
    }

    /// 容错扫描全部 shard；单个 shard 损坏不阻断其他只读投影。
    pub fn scan_all(&self, root: &Path) -> Result<ThreadEventScan, String> {
        let mut scan = ThreadEventScan::default();
        for entry in Self::event_files(root)? {
            match Self::read_file(root, &entry.path()) {
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
    /// 有临时文件却一条事件都读不到，说明重写崩在了替换分片的中间：读侧必须先自愈一次，
    /// 清不掉才报错，不能把它当成「从不存在」的新会话重建。
    pub fn thread_has_unfinished_write(root: &Path, thread_id: &str) -> bool {
        SHARD_STORE
            .has_stale_rewrite_temporaries(Self::thread_logical_path(root, thread_id))
            .unwrap_or(false)
    }

    /// 锁内自愈一次未完成重写的现场；清不掉时把错误交给读侧。
    fn heal_unfinished_write(root: &Path, thread_id: &str) -> Result<(), String> {
        SHARD_STORE
            .recover_unfinished_rewrite(Self::thread_logical_path(root, thread_id))
            .map_err(|_| format!("无法清理 thread {thread_id} 未完成重写的临时文件"))
    }

    /// 分片序号必须从 0 起连续。
    ///
    /// 缺口是重写替换到一半崩溃（或手工删块）的现场：按剩余分片继续投影只会安静地少一段
    /// 历史，因此读侧如实报不完整，由调用方决定修复还是重建。
    fn ensure_shards_contiguous(root: &Path, thread_id: &str) -> Result<(), String> {
        let sequences = SHARD_STORE
            .shard_sequences(Self::thread_logical_path(root, thread_id))
            .map_err(|_| "无法读取 thread 事件 shard".to_owned())?;
        for (expected, sequence) in sequences.iter().enumerate() {
            let expected = u64::try_from(expected).unwrap_or(u64::MAX);
            if *sequence != expected {
                return Err(format!(
                    "thread {thread_id} 的事件分片不完整：缺少序号 {expected}（下一步读到 {sequence}）"
                ));
            }
        }
        Ok(())
    }

    fn read_file(root: &Path, path: &Path) -> Result<Vec<ThreadEventRecord>, String> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(path).map_err(|_| "无法读取 thread 事件".to_owned())?;
        Self::parse_lines(root, &bytes)
    }

    fn parse_lines(root: &Path, bytes: &[u8]) -> Result<Vec<ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for (index, raw_line) in bytes.split(|byte| *byte == b'\n').enumerate() {
            let raw_line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
            if raw_line.is_empty() {
                continue;
            }
            let line = String::from_utf8(raw_line.to_vec())
                .map_err(|_| format!("thread 事件第 {} 行 UTF-8 无效", index + 1))?;
            let trimmed = line.trim();
            let record = Self::decode_record_line(root, trimmed.as_bytes())
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
        let line = Self::encode_record_line(root, record)?;
        SHARD_STORE
            .append_line(Self::thread_logical_path(root, &record.thread_id), &line)
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
        SHARD_STORE
            .append_with(logical, |lines| {
                let current =
                    Self::parse_lines(root, &Self::join_lines(lines)).map_err(std::io::Error::other)?;
                let actual = current.last().map_or(0, |event| event.sequence);
                if actual != expected_sequence {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "thread event sequence conflict: expected={expected_sequence} actual={actual}"
                        ),
                    ));
                }
                let line = Self::encode_record_line(root, record).map_err(std::io::Error::other)?;
                Ok(((), vec![line]))
            })
            .map_err(|error| error.to_string())?;
        super::thread_index::append_index_patch(root, record);
        Ok(())
    }

    fn read_thread(&self, root: &Path, thread_id: &str) -> Result<Vec<ThreadEventRecord>, String> {
        // 只读本 thread 的分片：trait 默认实现会 read_all（整语料），恢复与会话命令
        // 一旦走到那条路径，单次读取就退化成 O(全部会话)。
        //
        // 读之前先做两件事：分片序号必须连续（缺口＝历史缺一段），以及「一条事件都没有
        // 却留着重写临时文件」的崩溃现场要锁内自愈一次再重读。清不掉时把错误上抛。
        Self::ensure_shards_contiguous(root, thread_id)?;
        let records = Self::read_thread_full(root, thread_id)?;
        if !records.is_empty() || !Self::thread_has_unfinished_write(root, thread_id) {
            return Ok(records);
        }
        Self::heal_unfinished_write(root, thread_id)?;
        Self::ensure_shards_contiguous(root, thread_id)?;
        Self::read_thread_full(root, thread_id)
    }

    fn read_all(&self, root: &Path) -> Result<Vec<ThreadEventRecord>, String> {
        let mut records = Vec::new();
        for entry in Self::event_files(root)? {
            records.extend(Self::read_file(root, &entry.path())?);
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
                let current = Self::parse_lines(root, &bytes)
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
                let line = Self::encode_record_line(root, checkpoint).map_err(std::io::Error::other)?;
                Ok(((), vec![line]))
            })
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    error.to_string()
                } else {
                    "无法写入 thread checkpoint".to_owned()
                }
            })?;
        // checkpoint 替换整份历史：旧正文全部失去引用，只留这一条的正文。
        super::thread_event_body::prune_bodies_except(
            root,
            &checkpoint.thread_id,
            checkpoint.sequence,
        )
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
            let mut records = Self::read_file(root, path)?;
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

/// 单元测试独立成文：主文件贴 700 行硬限，测试不该占正文的行数。
#[cfg(test)]
#[path = "jsonl_store_tests.rs"]
mod tests;
