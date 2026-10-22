//! 会话级文件观察记录。
//!
//! `read_file` 与 `search` 负责记录模型真正看到的行；编辑工具只依据这里保存的
//! 完整内容哈希和历史版本做 seen-line 门禁与 stale anchor 恢复。

use std::{
    collections::{BTreeSet, HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use axon_crypto::sha256_hex;

pub(crate) const WORKSPACE_FILE_SNAPSHOT_MAX_BYTES: usize = 4 * 1024 * 1024;
const SNAPSHOT_MAX_PATHS: usize = 30;
const SNAPSHOT_MAX_VERSIONS_PER_PATH: usize = 4;
const SNAPSHOT_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// 一次可用于编辑校验的文件观察。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceFileSnapshot {
    path: PathBuf,
    content: Arc<str>,
    sha256: String,
    tag: String,
    seen_lines: BTreeSet<usize>,
    sequence: u64,
}

impl WorkspaceFileSnapshot {
    /// 返回规范化路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 返回该版本的完整文本。
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
    }

    /// 返回完整 SHA-256。
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// 返回模型可见的紧凑标签。
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// 返回模型在该版本上实际看过的 1-based 行号。
    #[must_use]
    pub fn seen_lines(&self) -> &BTreeSet<usize> {
        &self.seen_lines
    }
}

/// 快照标签解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceFileSnapshotLookup {
    /// 唯一匹配。
    Found(WorkspaceFileSnapshot),
    /// 没有匹配的历史版本。
    Missing,
    /// 紧凑标签碰撞，无法安全确定版本。
    Ambiguous,
}

/// 会话级、容量有界的文件观察存储。
#[derive(Debug, Default)]
pub struct WorkspaceFileSession {
    state: Mutex<WorkspaceFileSessionState>,
}

#[derive(Debug, Default)]
struct WorkspaceFileSessionState {
    versions: HashMap<PathBuf, VecDeque<Arc<WorkspaceFileSnapshot>>>,
    last_used: HashMap<PathBuf, u64>,
    sequence: u64,
    total_bytes: usize,
}

impl WorkspaceFileSession {
    /// 记录完整文件内容及本次真实展示的 1-based 行号。
    pub fn record_read(
        &self,
        path: impl Into<PathBuf>,
        content: impl Into<String>,
        seen_lines: impl IntoIterator<Item = usize>,
    ) -> Option<WorkspaceFileSnapshot> {
        let path = path.into();
        let content = content.into();
        if content.len() > WORKSPACE_FILE_SNAPSHOT_MAX_BYTES {
            return None;
        }
        let seen_lines = seen_lines.into_iter().filter(|line| *line > 0).collect();
        let sha256 = sha256_hex(content.as_bytes());
        let tag = crate::hashline_snapshot_tag(&content);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;

        if let Some(existing) = state
            .versions
            .get(&path)
            .and_then(VecDeque::front)
            .filter(|snapshot| snapshot.sha256 == sha256)
            .cloned()
        {
            let mut merged = existing.seen_lines.clone();
            merged.extend(seen_lines);
            let snapshot = WorkspaceFileSnapshot {
                path: path.clone(),
                content: Arc::clone(&existing.content),
                sha256,
                tag,
                seen_lines: merged,
                sequence,
            };
            state
                .versions
                .get_mut(&path)
                .expect("快照路径应仍存在")
                .push_front(Arc::new(snapshot.clone()));
            state
                .versions
                .get_mut(&path)
                .expect("快照路径应仍存在")
                .remove(1);
            state.last_used.insert(path, sequence);
            return Some(snapshot);
        }

        let snapshot = WorkspaceFileSnapshot {
            path: path.clone(),
            content: Arc::<str>::from(content),
            sha256,
            tag,
            seen_lines,
            sequence,
        };
        let bytes = snapshot.content.len();
        let versions = state.versions.entry(path.clone()).or_default();
        versions.push_front(Arc::new(snapshot.clone()));
        state.total_bytes = state.total_bytes.saturating_add(bytes);
        trim_path_versions(&mut state, &path);
        state.last_used.insert(path, sequence);
        evict_session(&mut state);
        Some(snapshot)
    }

    /// 按路径和可见标签解析模型引用的历史版本。
    #[must_use]
    pub fn lookup(&self, path: &Path, tag: &str) -> WorkspaceFileSnapshotLookup {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let matches = state
            .versions
            .get(path)
            .into_iter()
            .flatten()
            .filter(|snapshot| snapshot.tag.eq_ignore_ascii_case(tag))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => WorkspaceFileSnapshotLookup::Missing,
            [snapshot] => WorkspaceFileSnapshotLookup::Found((***snapshot).clone()),
            snapshots
                if snapshots
                    .windows(2)
                    .all(|pair| pair[0].sha256 == pair[1].sha256) =>
            {
                WorkspaceFileSnapshotLookup::Found((**snapshots[0]).clone())
            }
            _ => WorkspaceFileSnapshotLookup::Ambiguous,
        }
    }

    /// 按可见标签和文件名返回历史快照候选，按最近使用顺序排列。
    #[must_use]
    pub fn find_by_tag_and_file_name(
        &self,
        tag: &str,
        file_name: &str,
    ) -> Vec<WorkspaceFileSnapshot> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut matches = state
            .versions
            .values()
            .flatten()
            .filter(|snapshot| {
                snapshot.tag.eq_ignore_ascii_case(tag)
                    && snapshot
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name == file_name)
            })
            .map(|snapshot| (snapshot.sequence, (**snapshot).clone()))
            .collect::<Vec<_>>();
        matches.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        matches.into_iter().map(|(_, snapshot)| snapshot).collect()
    }

    /// 文件被不可观察的外部写入替换后，丢弃该路径的所有历史。
    pub fn invalidate(&self, path: &Path) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(versions) = state.versions.remove(path) {
            let released = versions
                .iter()
                .map(|snapshot| snapshot.content.len())
                .sum::<usize>();
            state.total_bytes = state.total_bytes.saturating_sub(released);
        }
        state.last_used.remove(path);
    }
}

fn trim_path_versions(state: &mut WorkspaceFileSessionState, path: &Path) {
    let Some(versions) = state.versions.get_mut(path) else {
        return;
    };
    let mut released = 0usize;
    while versions.len() > SNAPSHOT_MAX_VERSIONS_PER_PATH {
        if let Some(snapshot) = versions.pop_back() {
            released = released.saturating_add(snapshot.content.len());
        }
    }
    state.total_bytes = state.total_bytes.saturating_sub(released);
}

fn evict_session(state: &mut WorkspaceFileSessionState) {
    while state.versions.len() > SNAPSHOT_MAX_PATHS || state.total_bytes > SNAPSHOT_MAX_TOTAL_BYTES
    {
        let Some(path) = state
            .last_used
            .iter()
            .min_by_key(|(_, sequence)| **sequence)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        if let Some(versions) = state.versions.remove(&path) {
            let released = versions
                .iter()
                .map(|snapshot| snapshot.content.len())
                .sum::<usize>();
            state.total_bytes = state.total_bytes.saturating_sub(released);
        }
        state.last_used.remove(&path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_content_reads_merge_seen_lines() {
        let session = WorkspaceFileSession::default();
        session.record_read("src/lib.rs", "one\ntwo\nthree\n", [1, 2]);
        let latest = session
            .record_read("src/lib.rs", "one\ntwo\nthree\n", [3])
            .expect("快照应存在");

        assert_eq!(latest.seen_lines(), &BTreeSet::from([1, 2, 3]));
        assert!(matches!(
            session.lookup(Path::new("src/lib.rs"), latest.tag()),
            WorkspaceFileSnapshotLookup::Found(_)
        ));
    }

    #[test]
    fn stale_versions_remain_resolvable_for_recovery() {
        let session = WorkspaceFileSession::default();
        let before = session
            .record_read("src/lib.rs", "before\n", [1])
            .expect("旧快照应存在");
        session.record_read("src/lib.rs", "after\n", [1]);

        let lookup = session.lookup(Path::new("src/lib.rs"), before.tag());
        assert!(
            matches!(lookup, WorkspaceFileSnapshotLookup::Found(snapshot) if snapshot.content() == "before\n")
        );
    }

    #[test]
    fn oversized_files_do_not_create_editable_snapshots() {
        let session = WorkspaceFileSession::default();
        let oversized = "x".repeat(WORKSPACE_FILE_SNAPSHOT_MAX_BYTES + 1);

        assert!(session.record_read("large.txt", oversized, [1]).is_none());
        assert_eq!(
            session.lookup(Path::new("large.txt"), "0000"),
            WorkspaceFileSnapshotLookup::Missing
        );
    }
}
