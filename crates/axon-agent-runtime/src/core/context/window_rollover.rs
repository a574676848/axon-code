//! 上下文代际窗口轮转引擎（Context Window Rollover Engine）。
//!
//! 以本地 checkpoint 开启新的记忆窗口：
//! 1. 不发起额外模型请求；
//! 2. 推进窗口身份与 SessionNotes revision；
//! 3. 将旧 provider-visible history 按单消息 JSONL 归档；
//! 4. replacement history 由上层统一校验后再提交归档。

use std::path::Path;

use axon_crypto::sha256_hex;
use axon_domain::{AxonError, AxonResult};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent_model_message::AgentModelMessage;
use crate::session_notes::{SESSION_NOTES_FILE_NAME, SessionNotes};
use crate::window_rollover_storage::{
    ArchiveReverseScanReport, append_archive_records, archive_lifecycle_matches,
    read_archive_records, read_archive_snapshot, restore_archive_snapshot,
    visit_archive_records_reverse,
};

/// 冷存归档历史目录名（分片存储：archived_history/window_{n}.jsonl）。
pub const ARCHIVED_HISTORY_DIR_NAME: &str = "archived_history";
/// 归档窗口逻辑文件名前缀。
const ARCHIVED_WINDOW_FILE_PREFIX: &str = "window_";
/// 尚未发生窗口轮转时的缺省窗口 ID。
pub const DEFAULT_WINDOW_ID: &str = "window_0";
pub(crate) const WINDOW_ROLLOVER_LOCK_FILE_NAME: &str = ".window_rollover.lock";
/// 归档记录 schema 版本。
pub const ARCHIVED_WINDOW_SCHEMA_VERSION: u32 = 1;

/// 生成分片文件名。
#[must_use]
pub fn archive_shard_file_name(window_number: u64) -> String {
    format!("{ARCHIVED_WINDOW_FILE_PREFIX}{window_number}.jsonl")
}

/// 单条代际归档冷历史记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchivedWindowRecord {
    /// 归档记录 schema 版本。
    pub schema_version: u32,
    /// 本次轮转生命周期幂等键。
    pub lifecycle_id: String,
    /// 归档记录内容校验和（sha256 hex）。
    pub checksum: String,
    /// 归档所在的代际窗口序号。
    pub window_number: u64,
    /// 窗口全局唯一 ID。
    pub window_id: String,
    /// 上一代际窗口 ID。
    pub previous_window_id: Option<String>,
    /// 归档时间（Unix 毫秒）。
    pub archived_at_ms: i64,
    /// 归档消息。
    pub message: AgentModelMessage,
    /// 消息在原窗口中的绝对序号。
    pub message_ordinal: u64,
}

/// 已完成 replacement history 构建后的窗口归档结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowArchiveResult {
    /// 新代际窗口序号。
    pub window_number: u64,
    /// 新代际窗口 ID。
    pub window_id: String,
    /// 上一代际窗口 ID。
    pub previous_window_id: Option<String>,
    /// 本次窗口轮转提交后的 notes revision。
    pub notes_revision: u64,
    /// 归档消息数量。
    pub archived_messages_count: usize,
    /// 归档耗时（毫秒）。
    pub duration_ms: u64,
}

pub(crate) fn archive_checksum(record: &ArchivedWindowRecord) -> String {
    let mut unsigned = record.clone();
    unsigned.checksum.clear();
    serde_json::to_vec(&unsigned)
        .map(sha256_hex)
        .unwrap_or_default()
}

pub(crate) fn with_checksum(mut record: ArchivedWindowRecord) -> ArchivedWindowRecord {
    record.checksum = archive_checksum(&record);
    record
}

fn active_window_records(
    window_number: u64,
    current_window_id: Option<String>,
    previous_window_id: Option<String>,
    lifecycle_id: &str,
    active_messages: &[AgentModelMessage],
    archived_at_ms: i64,
) -> Vec<ArchivedWindowRecord> {
    let window_id = current_window_id.unwrap_or_else(|| DEFAULT_WINDOW_ID.to_owned());
    active_messages
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, message)| {
            with_checksum(ArchivedWindowRecord {
                schema_version: ARCHIVED_WINDOW_SCHEMA_VERSION,
                lifecycle_id: lifecycle_id.to_owned(),
                checksum: String::new(),
                window_number,
                window_id: window_id.clone(),
                previous_window_id: previous_window_id.clone(),
                archived_at_ms,
                message,
                message_ordinal: u64::try_from(ordinal).unwrap_or(u64::MAX),
            })
        })
        .collect()
}

/// 代际窗口轮转引擎。
pub struct WindowRolloverEngine;

impl WindowRolloverEngine {
    /// 仅归档旧窗口并推进 Session Notes，不构造替代历史消息。
    pub fn archive_active_window(
        session_dir: &Path,
        current_window_number: u64,
        current_window_id: Option<String>,
        expected_revision: u64,
        lifecycle_id: &str,
        active_messages: &[AgentModelMessage],
        now_ms: i64,
    ) -> AxonResult<WindowArchiveResult> {
        let start_instant = std::time::Instant::now();
        std::fs::create_dir_all(session_dir)
            .map_err(|e| AxonError::runtime_failure(format!("创建会话轮转目录失败: {e:?}")))?;
        let lock_path = session_dir.join(WINDOW_ROLLOVER_LOCK_FILE_NAME);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| AxonError::runtime_failure(format!("打开窗口轮转锁失败: {e:?}")))?;
        lock_file
            .lock()
            .map_err(|e| AxonError::runtime_failure(format!("获取窗口轮转锁失败: {e:?}")))?;
        let new_window_number = current_window_number.saturating_add(1).max(1);
        let new_window_id = Uuid::new_v4().to_string();
        let result = (|| {
            let mut notes = SessionNotes::load_from_session(session_dir)?
                .unwrap_or_else(|| SessionNotes::new(Default::default(), now_ms));
            if notes.last_rollover_lifecycle_id.as_deref() == Some(lifecycle_id) {
                if notes.previous_window_id != current_window_id {
                    return Err(AxonError::runtime_failure(
                        "窗口轮转 lifecycle 与已提交前序窗口身份不一致",
                    ));
                }
                let notes_revision = notes.last_rollover_revision.ok_or_else(|| {
                    AxonError::runtime_failure("窗口轮转记录缺少已提交 notes revision")
                })?;
                let expected_archive = active_window_records(
                    current_window_number,
                    current_window_id.clone(),
                    notes.last_archived_previous_window_id.clone(),
                    lifecycle_id,
                    active_messages,
                    now_ms,
                );
                if !archive_lifecycle_matches(session_dir, &expected_archive)? {
                    return Err(AxonError::runtime_failure(
                        "窗口轮转已推进但对应归档消息集合缺失",
                    ));
                }
                return Ok(WindowArchiveResult {
                    window_number: notes.window_number,
                    window_id: notes.current_window_id.clone().unwrap_or_default(),
                    previous_window_id: notes.previous_window_id.clone(),
                    notes_revision,
                    archived_messages_count: active_messages.len(),
                    duration_ms: u64::try_from(start_instant.elapsed().as_millis())
                        .unwrap_or(u64::MAX),
                });
            }
            if notes.window_number > current_window_number {
                return Err(AxonError::runtime_failure(format!(
                    "窗口轮转版本冲突：期望当前窗口 #{}，实际为 #{}",
                    current_window_number, notes.window_number
                )));
            }
            if notes.revision != expected_revision {
                return Err(AxonError::runtime_failure(format!(
                    "SessionNotes revision 冲突：期望 {}，实际 {}",
                    expected_revision, notes.revision
                )));
            }
            if notes.current_window_id != current_window_id {
                return Err(AxonError::runtime_failure(
                    "SessionNotes 当前窗口身份在提交前发生变化",
                ));
            }
            let archived_previous_window_id = notes.previous_window_id.clone();
            let previous_archive = read_archive_snapshot(session_dir, current_window_number)?;
            if !active_messages.is_empty() {
                let records = active_window_records(
                    current_window_number,
                    current_window_id.clone(),
                    archived_previous_window_id.clone(),
                    lifecycle_id,
                    active_messages,
                    now_ms,
                );
                // 记录在本函数内已带校验和，直接批量提交，避免再次整批克隆并重算校验和。
                if let Err(append_error) = append_archive_records(session_dir, &records) {
                    let rollback_result = restore_archive_snapshot(
                        session_dir,
                        current_window_number,
                        previous_archive.as_deref(),
                    );
                    return Err(match rollback_result {
                        Ok(()) => append_error,
                        Err(rollback_error) => AxonError::runtime_failure(format!(
                            "写入窗口归档失败且归档恢复失败：append={append_error:?}，rollback={rollback_error:?}"
                        )),
                    });
                }
            }
            notes.window_number = new_window_number;
            notes.previous_window_id = current_window_id.clone();
            notes.current_window_id = Some(new_window_id.clone());
            if notes.first_window_id.is_none() {
                notes.first_window_id = Some(
                    current_window_id
                        .clone()
                        .unwrap_or_else(|| new_window_id.clone()),
                );
            }
            notes.revision = notes.revision.saturating_add(1);
            notes.last_rollover_lifecycle_id = Some(lifecycle_id.to_owned());
            notes.last_archived_previous_window_id = archived_previous_window_id;
            notes.last_rollover_revision = Some(notes.revision);
            notes.updated_at_ms = now_ms;
            if let Err(save_error) = notes.save_to_session(session_dir) {
                let rollback_result = if active_messages.is_empty() {
                    Ok(())
                } else {
                    restore_archive_snapshot(
                        session_dir,
                        current_window_number,
                        previous_archive.as_deref(),
                    )
                };
                return Err(match rollback_result {
                    Ok(()) => save_error,
                    Err(rollback_error) => AxonError::runtime_failure(format!(
                        "保存窗口 SessionNotes 失败且归档恢复失败：notes={save_error:?}，archive={rollback_error:?}"
                    )),
                });
            }
            Ok(WindowArchiveResult {
                window_number: new_window_number,
                window_id: new_window_id.clone(),
                previous_window_id: current_window_id.clone(),
                notes_revision: notes.revision,
                archived_messages_count: active_messages.len(),
                duration_ms: u64::try_from(start_instant.elapsed().as_millis()).unwrap_or(u64::MAX),
            })
        })();
        drop(lock_file);
        result
    }

    /// 将单消息归档记录批量写入对应窗口分片。
    pub fn append_to_archive(
        session_dir: &Path,
        records: &[ArchivedWindowRecord],
    ) -> AxonResult<()> {
        let records = records
            .iter()
            .cloned()
            .map(with_checksum)
            .collect::<Vec<_>>();
        append_archive_records(session_dir, &records)
    }

    /// 读取指定窗口分片中的全部归档记录（历史回查的生产读取路径）。
    pub fn read_archive_shard(
        session_dir: &Path,
        window_number: u64,
    ) -> AxonResult<Vec<ArchivedWindowRecord>> {
        read_archive_records(session_dir, window_number)
    }

    /// 仅供存储测试读取完整分片；生产回查统一使用逆序流式扫描。
    #[cfg(test)]
    fn read_archive(
        session_dir: &Path,
        filter_window: Option<u64>,
    ) -> AxonResult<Vec<ArchivedWindowRecord>> {
        let archive_dir = session_dir.join(ARCHIVED_HISTORY_DIR_NAME);
        if let Some(target_w) = filter_window {
            return Self::read_archive_shard(session_dir, target_w);
        }

        if !archive_dir.exists() {
            return Ok(Vec::new());
        }

        let mut shard_windows = Self::list_archive_shards(session_dir)?;
        shard_windows.sort_unstable();

        let mut records = Vec::new();
        for w in shard_windows {
            records.extend(Self::read_archive_shard(session_dir, w)?);
        }
        Ok(records)
    }

    /// 按消息新近度逆序流式扫描指定窗口分片。
    pub(crate) fn visit_archive_reverse(
        session_dir: &Path,
        window_number: u64,
        visitor: &mut dyn FnMut(&ArchivedWindowRecord) -> bool,
    ) -> AxonResult<ArchiveReverseScanReport> {
        visit_archive_records_reverse(session_dir, window_number, visitor)
    }

    /// 在后续 projection 失败时回滚本次窗口提交。
    ///
    /// 仅允许回滚仍然由 `lifecycle_id` 持有的最新窗口；若期间已有其它轮转，
    /// 返回冲突而不是覆盖后续 notes 或归档事实。
    pub fn rollback_active_window(
        session_dir: &Path,
        current_window_number: u64,
        lifecycle_id: &str,
        committed_notes_revision: u64,
        previous_notes: Option<&SessionNotes>,
        previous_archive: Option<&[u8]>,
    ) -> AxonResult<()> {
        let lock_path = session_dir.join(WINDOW_ROLLOVER_LOCK_FILE_NAME);
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| AxonError::runtime_failure(format!("打开窗口回滚锁失败: {e:?}")))?;
        lock_file
            .lock()
            .map_err(|e| AxonError::runtime_failure(format!("获取窗口回滚锁失败: {e:?}")))?;
        let result = (|| {
            let notes_path = session_dir.join(SESSION_NOTES_FILE_NAME);
            let current_notes = SessionNotes::load_from_session(session_dir)?
                .ok_or_else(|| AxonError::runtime_failure("窗口回滚缺少当前 SessionNotes"))?;
            if current_notes.window_number != current_window_number.saturating_add(1)
                || current_notes.last_rollover_lifecycle_id.as_deref() != Some(lifecycle_id)
                || current_notes.revision != committed_notes_revision
                || current_notes.last_rollover_revision != Some(committed_notes_revision)
            {
                return Err(AxonError::runtime_failure(
                    "窗口回滚检测到后续生命周期或新任务状态，拒绝覆盖新事实",
                ));
            }

            restore_archive_snapshot(session_dir, current_window_number, previous_archive)?;
            if let Some(notes) = previous_notes {
                notes.save_to_session(session_dir)?;
            } else {
                if let Err(error) = std::fs::remove_file(&notes_path)
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    return Err(AxonError::runtime_failure(format!(
                        "删除本次 SessionNotes 失败: {error:?}"
                    )));
                }
            }
            Ok(())
        })();
        drop(lock_file);
        result
    }

    /// 读取指定窗口的归档回滚快照；损坏分片显式失败。
    pub fn read_archive_snapshot(
        session_dir: &Path,
        window_number: u64,
    ) -> AxonResult<Option<Vec<u8>>> {
        read_archive_snapshot(session_dir, window_number)
    }

    /// 列出所有可用的归档窗口分片序号（升序排序）。
    pub fn list_archive_shards(session_dir: &Path) -> AxonResult<Vec<u64>> {
        let archive_dir = session_dir.join(ARCHIVED_HISTORY_DIR_NAME);
        if !archive_dir.exists() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&archive_dir)
            .map_err(|e| AxonError::runtime_failure(format!("读取归档目录失败: {e:?}")))?;
        let mut windows = Vec::new();
        for entry_res in entries {
            let entry = entry_res
                .map_err(|e| AxonError::runtime_failure(format!("读取归档目录项失败: {e:?}")))?;
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();
            if let Some(w) = name_str
                .strip_prefix(ARCHIVED_WINDOW_FILE_PREFIX)
                .and_then(|remainder| remainder.split_once('.'))
                .and_then(|(window_number, shard_suffix)| {
                    parse_physical_shard_window(window_number, shard_suffix)
                })
            {
                windows.push(w);
            }
        }
        windows.sort_unstable();
        Ok(windows)
    }
}

/// 解析统一物理 shard 名称中的窗口序号，例如 `1` + `000000.jsonl`。
fn parse_physical_shard_window(window_number: &str, shard_suffix: &str) -> Option<u64> {
    let sequence = shard_suffix.strip_suffix(".jsonl")?;
    if sequence.len() != axon_jsonl::SHARD_SEQUENCE_WIDTH || sequence.parse::<u64>().is_err() {
        return None;
    }
    window_number.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    fn test_record(window_number: u64, content: &str) -> ArchivedWindowRecord {
        ArchivedWindowRecord {
            schema_version: ARCHIVED_WINDOW_SCHEMA_VERSION,
            lifecycle_id: format!("lifecycle-{window_number}-{content}"),
            checksum: String::new(),
            window_number,
            window_id: format!("window-{window_number}"),
            previous_window_id: None,
            archived_at_ms: 1,
            message: AgentModelMessage::text("user", content),
            message_ordinal: 0,
        }
    }

    #[test]
    fn concurrent_archive_appends_remain_valid_jsonl() {
        let directory = axon_test_support::test_temp_dir();
        let session_dir = Arc::new(directory.path().to_path_buf());
        let workers = (0..8)
            .map(|index| {
                let session_dir = Arc::clone(&session_dir);
                thread::spawn(move || {
                    WindowRolloverEngine::append_to_archive(
                        &session_dir,
                        &[test_record(1, &format!("message-{index}"))],
                    )
                    .expect("并发归档写入应成功");
                })
            })
            .collect::<Vec<_>>();

        for worker in workers {
            worker.join().expect("并发写入线程不应 panic");
        }

        let records = WindowRolloverEngine::read_archive(session_dir.as_ref(), Some(1))
            .expect("归档 JSONL 应保持可读");
        assert_eq!(records.len(), 8);
    }

    #[test]
    fn malformed_archive_line_returns_explicit_error() {
        let directory = axon_test_support::test_temp_dir();
        let archive_dir = directory.path().join(ARCHIVED_HISTORY_DIR_NAME);
        std::fs::create_dir_all(&archive_dir).expect("归档目录应创建成功");
        let logical = archive_dir.join(archive_shard_file_name(1));
        let shard = axon_jsonl::ShardedJsonlStore::physical_shard_path(&logical, 0);
        std::fs::write(&shard, b"{invalid-json}\n").expect("损坏 fixture 应写入");
        let error = WindowRolloverEngine::read_archive(directory.path(), Some(1))
            .expect_err("损坏归档行必须显式报错");
        assert!(format!("{error:?}").contains("第 1 行"));
    }
}
