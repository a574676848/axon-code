//! Thread 派生索引：把会话级读数（父子关系、标题、来源、状态与末轮时间）单独成行。
//!
//! 索引是**派生缓存**，不是第二事实源：thread 事件流仍是唯一真相，索引缺失或损坏时
//! 调用方回退整表投影，`rebuild` 可随时从事件流重建。它存在的理由只有一条：名册、
//! 保留策略与用量水位都需要「全体会话的少量标量」，逐会话解析整条历史会把这三条路径
//! 变成 O(语料) 的 JSON 反序列化。
//!
//! 写入点只有一个：`ThreadEventStore::append` 成功后追加一行补丁（失败只丢缓存，不影响
//! 事实写入）。每行只带本次事件改变的字段，读取时按 `thread_id` 逐行折叠。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::domain::{ThreadEvent, ThreadEventRecord, ThreadStatus, TurnStatus};
use crate::infrastructure::jsonl_store::{JsonlThreadEventStore, SHARD_STORE};

/// 索引文件名；与 thread 事件分片同目录，便于一起归档与一起删除。
pub const THREAD_INDEX_FILE: &str = "thread-index.v1.jsonl";

/// 索引里的一个 thread 折叠结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThreadIndexEntry {
    /// Thread ID。
    pub thread_id: String,
    /// 父 thread ID。
    pub parent_thread_id: Option<String>,
    /// Thread 名称。
    pub name: Option<String>,
    /// 子会话来源。
    pub child_origin: Option<String>,
    /// 是否已归档。
    pub archived: bool,
    /// 最近更新时间（毫秒）。
    pub updated_at_ms: u64,
    /// 最近状态。
    pub status: ThreadStatus,
    /// 末轮状态。
    pub last_turn_status: Option<TurnStatus>,
    /// 末轮时间（毫秒）。
    pub last_turn_at_ms: Option<u64>,
    /// provider 名称。
    pub provider: String,
    /// 模型名称。
    pub model: String,
}

/// 索引读取结果：条目与最新活动时间。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadIndexSnapshot {
    /// 全部 thread 条目，按最近更新倒序。
    pub entries: Vec<ThreadIndexEntry>,
    /// 全部条目里最新的 `updated_at_ms`。
    pub max_updated_at_ms: u64,
}

/// 一行索引补丁：只带本次事件改变的字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadIndexPatch {
    /// thread ID。
    thread_id: String,
    /// 记录时间。
    at: u64,
    /// 父 thread；`Some(None)` 表示显式无父。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parent: Option<Option<String>>,
    /// 名称。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<Option<String>>,
    /// 子会话来源。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin: Option<Option<String>>,
    /// 是否归档。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archived: Option<bool>,
    /// 状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    status: Option<ThreadStatus>,
    /// 末轮状态。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_turn_status: Option<TurnStatus>,
    /// 末轮时间。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_turn_at_ms: Option<u64>,
    /// 删除标记：会话被保留策略清理后写一条，读取时整条移除。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
    /// provider。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    /// 模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
}

impl Default for ThreadIndexEntry {
    fn default() -> Self {
        Self {
            thread_id: String::new(),
            parent_thread_id: None,
            name: None,
            child_origin: None,
            archived: false,
            updated_at_ms: 0,
            status: ThreadStatus::Idle,
            last_turn_status: None,
            last_turn_at_ms: None,
            provider: String::new(),
            model: String::new(),
        }
    }
}

impl JsonlThreadEventStore {
    /// 索引逻辑路径：与 `threads/` 同级，避免被 thread 分片扫描当成事件文件。
    #[must_use]
    pub fn thread_index_path(root: &Path) -> PathBuf {
        Self::path(root)
            .parent()
            .map(|directory| directory.join(THREAD_INDEX_FILE))
            .unwrap_or_else(|| PathBuf::from(THREAD_INDEX_FILE))
    }

    /// 读取索引；索引缺失时返回 `None`，调用方据此回退整表投影。
    pub fn read_thread_index(&self, root: &Path) -> Result<Option<ThreadIndexSnapshot>, String> {
        let path = Self::thread_index_path(root);
        // 索引按 JSONL 分片写入（`thread-index.v1.000000.jsonl`），不能用逻辑路径判断存在。
        let shards = SHARD_STORE
            .shard_paths(&path)
            .map_err(|_| "无法读取 thread 索引".to_owned())?;
        if shards.is_empty() {
            return Ok(None);
        }
        let lines = SHARD_STORE
            .read_lines(&path)
            .map_err(|_| "无法读取 thread 索引".to_owned())?;
        let mut entries = BTreeMap::<String, ThreadIndexEntry>::new();
        for line in lines {
            if line.is_empty() {
                continue;
            }
            // 坏行只丢这一行的补丁：索引是缓存，不能因为它阻断检索。
            let Ok(patch) = serde_json::from_slice::<ThreadIndexPatch>(&line) else {
                continue;
            };
            apply_patch(&mut entries, patch);
        }
        let mut entries = entries.into_values().collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.thread_id.cmp(&right.thread_id))
        });
        let max_updated_at_ms = entries
            .iter()
            .map(|entry| entry.updated_at_ms)
            .max()
            .unwrap_or(0);
        Ok(Some(ThreadIndexSnapshot {
            entries,
            max_updated_at_ms,
        }))
    }

    /// 从事件流重建索引：先写临时文件再替换，崩溃不会留下半份索引。
    pub fn rebuild_thread_index(&self, root: &Path) -> Result<(), String> {
        let scan = self.scan_all(root)?;
        let mut entries = BTreeMap::<String, ThreadIndexEntry>::new();
        for record in &scan.records {
            if let Some(patch) = index_patch(record) {
                apply_patch(&mut entries, patch);
            }
        }
        let path = Self::thread_index_path(root);
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)
                .map_err(|_| "无法创建 thread 索引目录".to_owned())?;
        }
        let mut body = Vec::new();
        for entry in entries.into_values() {
            let patch = ThreadIndexPatch::from_entry(&entry);
            let line =
                serde_json::to_vec(&patch).map_err(|_| "thread 索引序列化失败".to_owned())?;
            body.extend_from_slice(&line);
            body.push(b'\n');
        }
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, &body).map_err(|_| "thread 索引写入失败".to_owned())?;
        std::fs::rename(&temporary, &path).map_err(|_| "thread 索引替换失败".to_owned())
    }
}

impl ThreadIndexPatch {
    fn from_entry(entry: &ThreadIndexEntry) -> Self {
        Self {
            thread_id: entry.thread_id.clone(),
            at: entry.updated_at_ms,
            parent: Some(entry.parent_thread_id.clone()),
            name: Some(entry.name.clone()),
            origin: Some(entry.child_origin.clone()),
            archived: Some(entry.archived),
            status: Some(entry.status),
            last_turn_status: entry.last_turn_status,
            last_turn_at_ms: entry.last_turn_at_ms,
            provider: Some(entry.provider.clone()),
            model: Some(entry.model.clone()),
            deleted: false,
        }
    }
}

/// 追加一条索引补丁；失败只丢缓存，不影响事件写入本身。
pub(crate) fn append_index_patch(root: &Path, record: &ThreadEventRecord) {
    let Some(patch) = index_patch(record) else {
        return;
    };
    let Ok(line) = serde_json::to_string(&patch) else {
        return;
    };
    let _ = SHARD_STORE.append_line(
        JsonlThreadEventStore::thread_index_path(root),
        line.as_bytes(),
    );
}

/// 会话被清理时同步写一条删除补丁，避免索引留下幽灵条目。
pub(crate) fn append_thread_removed(root: &Path, thread_id: &str) {
    let patch = ThreadIndexPatch {
        thread_id: thread_id.to_owned(),
        at: axon_time::unix_now_ms(),
        parent: None,
        name: None,
        origin: None,
        archived: None,
        status: None,
        last_turn_status: None,
        last_turn_at_ms: None,
        provider: None,
        model: None,
        deleted: true,
    };
    let Ok(line) = serde_json::to_string(&patch) else {
        return;
    };
    let _ = SHARD_STORE.append_line(
        JsonlThreadEventStore::thread_index_path(root),
        line.as_bytes(),
    );
}

fn index_patch(record: &ThreadEventRecord) -> Option<ThreadIndexPatch> {
    let at = record.recorded_at_ms;
    let mut patch = ThreadIndexPatch {
        thread_id: record.thread_id.clone(),
        at,
        parent: None,
        name: None,
        origin: None,
        archived: None,
        status: None,
        last_turn_status: None,
        last_turn_at_ms: None,
        provider: None,
        model: None,
        deleted: false,
    };
    match &record.event {
        ThreadEvent::ThreadStarted {
            parent_thread_id,
            settings,
            ..
        } => {
            patch.parent = Some(parent_thread_id.clone());
            patch.provider = Some(settings.provider.clone());
            patch.model = Some(settings.model.clone());
            patch.status = Some(ThreadStatus::Idle);
        }
        ThreadEvent::TurnStarted { .. } => {
            patch.status = Some(ThreadStatus::Active);
            patch.last_turn_status = Some(TurnStatus::Active);
            patch.last_turn_at_ms = Some(at);
        }
        ThreadEvent::TurnCompleted { turn } => {
            patch.status = Some(if turn.status == TurnStatus::Active {
                ThreadStatus::Active
            } else {
                ThreadStatus::Idle
            });
            patch.last_turn_status = Some(turn.status);
            patch.last_turn_at_ms = Some(turn.completed_at_ms.unwrap_or(at));
        }
        ThreadEvent::TurnFailed { turn_id, .. } => {
            patch.status = Some(ThreadStatus::Idle);
            patch.last_turn_status = Some(TurnStatus::Failed);
            patch.last_turn_at_ms = Some(at);
            let _ = turn_id;
        }
        ThreadEvent::TurnInterrupted { turn_id, .. } => {
            patch.status = Some(ThreadStatus::Idle);
            patch.last_turn_status = Some(TurnStatus::Interrupted);
            patch.last_turn_at_ms = Some(at);
            let _ = turn_id;
        }
        ThreadEvent::ThreadNamed { name } => {
            patch.name = Some(name.clone());
        }
        ThreadEvent::ThreadAnnotated { child_origin } => {
            patch.origin = Some(child_origin.clone());
        }
        ThreadEvent::ThreadArchived => {
            patch.archived = Some(true);
            patch.status = Some(ThreadStatus::Archived);
        }
        ThreadEvent::ThreadUnarchived => {
            patch.archived = Some(false);
            patch.status = Some(ThreadStatus::Idle);
        }
        ThreadEvent::ThreadClosed => {
            patch.status = Some(ThreadStatus::NotLoaded);
        }
        // 纯噪声事件不写索引：只有会话级标量变化才值得一行，否则索引会随 item 数量膨胀。
        ThreadEvent::TurnItemsAppended { .. }
        | ThreadEvent::TurnMetadataPatched { .. }
        | ThreadEvent::ThreadResumed
        | ThreadEvent::ThreadCompacted { .. }
        | ThreadEvent::ThreadCheckpoint { .. }
        | ThreadEvent::ThreadForked { .. } => return None,
    }
    Some(patch)
}

fn apply_patch(entries: &mut BTreeMap<String, ThreadIndexEntry>, patch: ThreadIndexPatch) {
    if patch.deleted {
        entries.remove(&patch.thread_id);
        return;
    }
    let entry = entries
        .entry(patch.thread_id.clone())
        .or_insert_with(|| ThreadIndexEntry {
            thread_id: patch.thread_id.clone(),
            status: ThreadStatus::Idle,
            ..ThreadIndexEntry::default()
        });
    if let Some(parent) = patch.parent {
        entry.parent_thread_id = parent;
    }
    if let Some(name) = patch.name {
        entry.name = name;
    }
    if let Some(origin) = patch.origin {
        entry.child_origin = origin;
    }
    if let Some(archived) = patch.archived {
        entry.archived = archived;
    }
    if let Some(status) = patch.status {
        entry.status = status;
    }
    if let Some(last_turn_status) = patch.last_turn_status {
        entry.last_turn_status = Some(last_turn_status);
    }
    if let Some(last_turn_at_ms) = patch.last_turn_at_ms {
        entry.last_turn_at_ms = Some(last_turn_at_ms);
    }
    if let Some(provider) = patch.provider {
        entry.provider = provider;
    }
    if let Some(model) = patch.model {
        entry.model = model;
    }
    entry.updated_at_ms = entry.updated_at_ms.max(patch.at);
}
