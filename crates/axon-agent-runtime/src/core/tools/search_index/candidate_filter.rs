use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use super::{
    model::{FileFingerprint, IndexSnapshot, TrigramKey, relative_path},
    overlay::LiveOverlay,
    planner::QueryPlan,
};
use crate::workspace_walker::WorkspaceWalkEntry;

#[derive(Debug, Default)]
pub(crate) struct CandidateSelection {
    excluded: BTreeSet<PathBuf>,
}

impl CandidateSelection {
    pub(crate) fn should_scan(&self, path: &Path) -> bool {
        !self.excluded.contains(path)
    }

    /// 被安全排除的文件数；只服务于测试与基准诊断。
    #[cfg(test)]
    pub(crate) fn excluded_count(&self) -> usize {
        self.excluded.len()
    }
}

/// 只排除「已被有效索引覆盖、确认可搜索且不含必需 trigram」的文件。
///
/// 任何未覆盖、指纹不一致、覆盖层标记为删除的文件都保持可扫描，保证 fail-open。
pub(crate) fn select_candidates(
    snapshot: &IndexSnapshot,
    overlay: &LiveOverlay,
    root: &Path,
    entries: &[WorkspaceWalkEntry],
    plan: &QueryPlan,
) -> CandidateSelection {
    let Some(possible) = possible_paths(snapshot, overlay, plan) else {
        return CandidateSelection::default();
    };

    let mut excluded = BTreeSet::new();
    for entry in entries.iter().filter(|entry| !entry.is_dir) {
        let Some(relative) = relative_path(root, &entry.path) else {
            continue;
        };
        let Some(searchable) = coverage(snapshot, overlay, &relative, entry) else {
            continue;
        };
        if searchable && !possible.contains(&relative) {
            excluded.insert(entry.path.clone());
        }
    }
    CandidateSelection { excluded }
}

/// 该路径本轮索引可信度：`Some(searchable)` 表示基础快照或覆盖层指纹一致，
/// `None` 表示未覆盖或指纹已变，必须保守扫描。
fn coverage(
    snapshot: &IndexSnapshot,
    overlay: &LiveOverlay,
    relative: &str,
    entry: &WorkspaceWalkEntry,
) -> Option<bool> {
    let fingerprint = FileFingerprint {
        length: entry.file_len,
        modified_at_ms: entry.modified_at_ms,
    };
    // 覆盖层为空是稳定期的常态；此时省掉每文件 2-3 次 map 查找。
    if !overlay.is_empty() {
        if let Some(searchable) = overlay.effective_entry(relative, &fingerprint) {
            return Some(searchable);
        }
        if overlay.is_removed(relative) || overlay.entry(relative).is_some() {
            return None;
        }
    }
    let indexed = snapshot.files.get(relative)?;
    (indexed.fingerprint == fingerprint).then_some(indexed.searchable)
}

fn possible_paths(
    snapshot: &IndexSnapshot,
    overlay: &LiveOverlay,
    plan: &QueryPlan,
) -> Option<BTreeSet<String>> {
    let QueryPlan::Required(required) = plan else {
        return None;
    };
    let mut keys = required.iter().collect::<Vec<_>>();
    keys.sort_by_key(|key| estimate(snapshot, overlay, key));
    let first = keys.first()?;

    let mut possible = candidates_for(snapshot, overlay, first);
    for key in keys.into_iter().skip(1) {
        if possible.is_empty() {
            break;
        }
        let candidates = candidates_for(snapshot, overlay, key);
        possible = intersect_sorted(&possible, &candidates);
    }
    Some(possible.into_iter().collect())
}

/// 估算某个 trigram 的候选规模，用于按 posting 长度从短到长做交集。
fn estimate(snapshot: &IndexSnapshot, overlay: &LiveOverlay, key: &TrigramKey) -> usize {
    snapshot.postings.get(key).map_or(0, Vec::len)
        + overlay.candidates(*key).map_or(0, BTreeSet::len)
}

/// 基础快照与覆盖层中该 trigram 的候选路径（升序去重）。
///
/// 已进入覆盖层或 tombstone 的路径不能再用基础快照的 posting 判定，因此在这里剔除。
fn candidates_for(
    snapshot: &IndexSnapshot,
    overlay: &LiveOverlay,
    key: &TrigramKey,
) -> Vec<String> {
    let mut paths = Vec::new();
    // posting 可能很长；覆盖层为空时不必对每个 id 做两次 map 查找。
    let overlay_is_empty = overlay.is_empty();
    if let Some(ids) = snapshot.postings.get(key) {
        for id in ids {
            let Some(path) = snapshot.path_of(*id) else {
                continue;
            };
            if !overlay_is_empty && (overlay.is_removed(path) || overlay.entry(path).is_some()) {
                continue;
            }
            paths.push(path.to_owned());
        }
    }
    if let Some(overlay_paths) = overlay.candidates(*key) {
        paths.extend(overlay_paths.iter().cloned());
    }
    paths.sort_unstable();
    paths.dedup();
    paths
}

fn intersect_sorted(left: &[String], right: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(left.len().min(right.len()));
    let mut left_index = 0;
    let mut right_index = 0;
    while left_index < left.len() && right_index < right.len() {
        match left[left_index].cmp(&right[right_index]) {
            std::cmp::Ordering::Less => left_index += 1,
            std::cmp::Ordering::Greater => right_index += 1,
            std::cmp::Ordering::Equal => {
                result.push(left[left_index].clone());
                left_index += 1;
                right_index += 1;
            }
        }
    }
    result
}
