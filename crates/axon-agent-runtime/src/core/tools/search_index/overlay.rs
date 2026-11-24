//! 内存增量覆盖层。
//!
//! 记录本轮 walker 观察到变化、但尚未合并进基础快照的文件：新增/修改写入 `entries`，
//! 删除写入 `tombstones`。查询时基础快照与覆盖层共同参与候选判定，因此编辑循环中
//! 只需要重读变化的少数文件，而不必重建整份索引。

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use super::{
    builder::index_file,
    model::{
        FileFingerprint, IndexSnapshot, IndexedFile, SearchIndexPolicy, TrigramKey, relative_path,
    },
};
use crate::workspace_walker::WorkspaceWalkEntry;

/// 单个覆盖条目：变化文件的指纹、可搜索标记和触发词集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OverlayEntry {
    pub(crate) fingerprint: FileFingerprint,
    pub(crate) searchable: bool,
    pub(crate) trigrams: Vec<TrigramKey>,
}

/// 基础快照之上的增量层。
#[derive(Debug, Default, Clone)]
pub(crate) struct LiveOverlay {
    entries: BTreeMap<String, OverlayEntry>,
    tombstones: BTreeSet<String>,
    postings: BTreeMap<TrigramKey, BTreeSet<String>>,
    bytes: u64,
}

impl LiveOverlay {
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.tombstones.is_empty()
    }

    pub(crate) fn entry(&self, path: &str) -> Option<&OverlayEntry> {
        self.entries.get(path)
    }

    pub(crate) fn is_removed(&self, path: &str) -> bool {
        self.tombstones.contains(path)
    }

    pub(crate) fn candidates(&self, trigram: TrigramKey) -> Option<&BTreeSet<String>> {
        self.postings.get(&trigram)
    }

    /// 已进入覆盖层的路径（不含 tombstone）。
    pub(crate) fn entry_paths(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }

    /// 覆盖层对该路径的有效判定：指纹一致时返回可搜索标记，否则返回 `None` 表示不可信。
    pub(crate) fn effective_entry(
        &self,
        path: &str,
        fingerprint: &FileFingerprint,
    ) -> Option<bool> {
        if self.tombstones.contains(path) {
            return None;
        }
        let entry = self.entries.get(path)?;
        (entry.fingerprint == *fingerprint).then_some(entry.searchable)
    }

    pub(crate) fn exceeds_budget(&self, policy: SearchIndexPolicy) -> bool {
        self.entries.len().saturating_add(self.tombstones.len()) > policy.max_overlay_files
            || self.bytes > policy.max_overlay_bytes
    }

    /// 读取一个发生变化的工作区文件并写入覆盖层；路径不属于 root 时直接忽略。
    pub(crate) fn ingest(
        &mut self,
        root: &Path,
        entry: &WorkspaceWalkEntry,
        policy: SearchIndexPolicy,
    ) {
        let Some(relative) = relative_path(root, &entry.path) else {
            return;
        };
        let data = index_file(entry, policy.max_file_size, policy.max_overlay_bytes);
        self.upsert(&relative, data.fingerprint, data.searchable, data.trigrams);
    }

    pub(crate) fn upsert(
        &mut self,
        path: &str,
        fingerprint: FileFingerprint,
        searchable: bool,
        trigrams: Vec<TrigramKey>,
    ) {
        self.drop_entry_index(path);
        self.tombstones.remove(path);
        if searchable {
            for trigram in &trigrams {
                self.postings
                    .entry(*trigram)
                    .or_default()
                    .insert(path.to_owned());
            }
        }
        self.bytes = self.bytes.saturating_add(entry_bytes(path, &trigrams));
        self.entries.insert(
            path.to_owned(),
            OverlayEntry {
                fingerprint,
                searchable,
                trigrams,
            },
        );
    }

    pub(crate) fn remove(&mut self, path: &str) {
        self.drop_entry_index(path);
        self.tombstones.insert(path.to_owned());
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(path.len()).unwrap_or(u64::MAX));
    }

    fn drop_entry_index(&mut self, path: &str) {
        let Some(entry) = self.entries.remove(path) else {
            return;
        };
        for trigram in &entry.trigrams {
            let empty = if let Some(paths) = self.postings.get_mut(trigram) {
                paths.remove(path);
                paths.is_empty()
            } else {
                false
            };
            if empty {
                self.postings.remove(trigram);
            }
        }
        self.bytes = self
            .bytes
            .saturating_sub(entry_bytes(path, &entry.trigrams));
    }

    /// 把覆盖层合并进基础快照，生成新的内存快照；不重新读取任何文件内容。
    pub(crate) fn merge_into(self, snapshot: &IndexSnapshot, generation: u64) -> IndexSnapshot {
        let LiveOverlay {
            entries,
            tombstones,
            ..
        } = self;
        let mut merged = snapshot.clone();

        let mut removed_ids = BTreeSet::new();
        for path in tombstones.iter().chain(entries.keys()) {
            if let Some(file) = snapshot.files.get(path) {
                removed_ids.insert(file.id);
            }
            merged.files.remove(path);
        }
        if !removed_ids.is_empty() {
            for ids in merged.postings.values_mut() {
                ids.retain(|id| !removed_ids.contains(id));
            }
            merged.postings.retain(|_, ids| !ids.is_empty());
        }

        let mut next_id = merged
            .files
            .values()
            .map(|file| file.id)
            .max()
            .map_or(0, |id| id.saturating_add(1));
        for (path, entry) in &entries {
            let id = next_id;
            next_id = next_id.saturating_add(1);
            if entry.searchable {
                for trigram in &entry.trigrams {
                    merged.postings.entry(*trigram).or_default().push(id);
                }
            }
            merged.files.insert(
                path.clone(),
                IndexedFile {
                    id,
                    fingerprint: entry.fingerprint.clone(),
                    searchable: entry.searchable,
                },
            );
        }

        for ids in merged.postings.values_mut() {
            ids.sort_unstable();
            ids.dedup();
        }

        merged.generation = generation;
        merged.rebuild_paths();
        merged
    }
}

fn entry_bytes(path: &str, trigrams: &[TrigramKey]) -> u64 {
    let per_trigram = u64::try_from(std::mem::size_of::<TrigramKey>()).unwrap_or(u64::MAX);
    u64::try_from(path.len())
        .unwrap_or(u64::MAX)
        .saturating_add(
            u64::try_from(trigrams.len())
                .unwrap_or(u64::MAX)
                .saturating_mul(per_trigram),
        )
}
