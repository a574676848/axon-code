//! Session Index - 线程命名历史索引层。
//!
//! 参考 codex-rs `rollout/src/session_index.rs` 设计，实现：
//! - Append-only JSONL 持久化
//! - 线程名称索引管理（thread_name <-> thread_id 映射）
//! - 从文件末尾倒序扫描查找最新记录
//! - 批量线程 ID 查询

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

/// Session index 文件名。
const SESSION_INDEX_FILE: &str = "session_index.jsonl";
/// Session index 使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

/// 线程 ID 类型别名。
pub type ThreadId = u64;

/// Session index 条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionIndexEntry {
    /// 线程 ID。
    pub id: ThreadId,
    /// 线程名称。
    pub thread_name: String,
    /// 更新时间戳（RFC3339 格式）。
    pub updated_at: String,
}

/// Session index runtime - 管理线程命名历史索引。
#[derive(Debug, Default, Clone, Copy)]
pub struct SessionIndexRuntime {
    /// 是否保持 ref-only 模式。
    ref_only: bool,
    /// 是否允许远控读取。
    remote_source_read_allowed: bool,
}

impl SessionIndexRuntime {
    /// 创建新实例。
    #[must_use]
    pub fn new() -> Self {
        Self {
            ref_only: true,
            remote_source_read_allowed: false,
        }
    }

    /// 设置 ref-only 模式。
    #[must_use]
    pub fn with_ref_only(mut self, ref_only: bool) -> Self {
        self.ref_only = ref_only;
        self
    }

    /// 设置远控源码读取许可。
    #[must_use]
    pub fn with_remote_source_read_allowed(mut self, allowed: bool) -> Self {
        self.remote_source_read_allowed = allowed;
        self
    }

    /// 追加线程名称到索引。
    ///
    /// 索引文件为 append-only，最近的记录优先。
    pub async fn append_thread_name(
        &self,
        codex_home: &Path,
        thread_id: ThreadId,
        name: &str,
    ) -> std::io::Result<()> {
        let updated_at = iso8601_timestamp();
        let entry = SessionIndexEntry {
            id: thread_id,
            thread_name: name.to_string(),
            updated_at,
        };
        self.append_entry(codex_home, &entry).await
    }

    /// 追加原始索引条目。
    pub async fn append_entry(
        &self,
        codex_home: &Path,
        entry: &SessionIndexEntry,
    ) -> std::io::Result<()> {
        let path = self.session_index_path(codex_home);
        let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        tokio::task::spawn_blocking(move || SHARD_STORE.append_line(path, line))
            .await
            .map_err(std::io::Error::other)?
    }

    /// 根据线程 ID 查找最新的线程名称。
    pub async fn find_thread_name_by_id(
        &self,
        codex_home: &Path,
        thread_id: ThreadId,
    ) -> std::io::Result<Option<String>> {
        Ok(read_entries_sync(codex_home)?
            .into_iter()
            .rev()
            .find(|entry| entry.id == thread_id)
            .map(|entry| entry.thread_name))
    }

    /// 根据线程 ID 集合批量查找线程名称。
    pub async fn find_thread_names_by_ids(
        &self,
        codex_home: &Path,
        thread_ids: &HashSet<ThreadId>,
    ) -> std::io::Result<HashMap<ThreadId, String>> {
        if thread_ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut names = HashMap::with_capacity(thread_ids.len());
        let path = self.session_index_path(codex_home);
        let entries = tokio::task::spawn_blocking(move || read_entries_at(&path))
            .await
            .map_err(std::io::Error::other)??;
        for entry in entries {
            let name = entry.thread_name.trim();
            if !name.is_empty() && thread_ids.contains(&entry.id) {
                names.insert(entry.id, name.to_string());
            }
        }

        Ok(names)
    }

    /// 读取索引文件所有条目。
    pub async fn read_all_entries(
        &self,
        codex_home: &Path,
    ) -> std::io::Result<Vec<SessionIndexEntry>> {
        let path = self.session_index_path(codex_home);
        tokio::task::spawn_blocking(move || read_entries_at(&path))
            .await
            .map_err(std::io::Error::other)?
    }

    /// 获取索引文件路径。
    fn session_index_path(&self, codex_home: &Path) -> PathBuf {
        codex_home.join(SESSION_INDEX_FILE)
    }
}

fn read_entries_sync(codex_home: &Path) -> std::io::Result<Vec<SessionIndexEntry>> {
    read_entries_at(&codex_home.join(SESSION_INDEX_FILE))
}

fn read_entries_at(path: &Path) -> std::io::Result<Vec<SessionIndexEntry>> {
    SHARD_STORE
        .read_lines(path)?
        .iter()
        .map(|line| parse_index_line(line))
        .collect()
}

fn parse_index_line(line: &[u8]) -> std::io::Result<SessionIndexEntry> {
    serde_json::from_slice(line).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// 生成 ISO8601 格式时间戳。
fn iso8601_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn create_temp_dir() -> std::path::PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let seq = TEMP_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            axon_test_support::test_temp_root().join(format!("axon-session-index-{unique}-{seq}"));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[tokio::test]
    async fn test_append_and_find() {
        let runtime = SessionIndexRuntime::new();
        let temp_dir = create_temp_dir();

        runtime
            .append_thread_name(&temp_dir, 1, "test-thread")
            .await
            .unwrap();

        let name = runtime.find_thread_name_by_id(&temp_dir, 1).await.unwrap();
        assert_eq!(name, Some("test-thread".to_string()));
    }

    #[tokio::test]
    async fn test_find_nonexistent() {
        let runtime = SessionIndexRuntime::new();
        let temp_dir = create_temp_dir();

        let name = runtime
            .find_thread_name_by_id(&temp_dir, 999)
            .await
            .unwrap();
        assert_eq!(name, None);
    }

    #[tokio::test]
    async fn test_latest_name_wins() {
        let runtime = SessionIndexRuntime::new();
        let temp_dir = create_temp_dir();

        runtime
            .append_thread_name(&temp_dir, 1, "name-v1")
            .await
            .unwrap();
        runtime
            .append_thread_name(&temp_dir, 1, "name-v2")
            .await
            .unwrap();

        let name = runtime.find_thread_name_by_id(&temp_dir, 1).await.unwrap();
        assert_eq!(name, Some("name-v2".to_string()));
    }

    #[tokio::test]
    async fn test_batch_find() {
        let runtime = SessionIndexRuntime::new();
        let temp_dir = create_temp_dir();

        runtime
            .append_thread_name(&temp_dir, 1, "thread-1")
            .await
            .unwrap();
        runtime
            .append_thread_name(&temp_dir, 2, "thread-2")
            .await
            .unwrap();
        runtime
            .append_thread_name(&temp_dir, 3, "thread-3")
            .await
            .unwrap();

        let ids: HashSet<ThreadId> = [1, 2].iter().copied().collect();
        let names = runtime
            .find_thread_names_by_ids(&temp_dir, &ids)
            .await
            .unwrap();

        assert_eq!(names.len(), 2);
        assert_eq!(names.get(&1), Some(&"thread-1".to_string()));
        assert_eq!(names.get(&2), Some(&"thread-2".to_string()));
    }
}
