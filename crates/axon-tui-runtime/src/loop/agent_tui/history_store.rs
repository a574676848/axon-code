//! Composer draft 历史持久化。

use std::collections::hash_map::DefaultHasher;
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
};

use axon_config::{USER_RUNTIME_DATA_DIR, USER_RUNTIME_SUBDIR};
use axon_jsonl::ShardedJsonlStore;
use serde::{Deserialize, Serialize};

const DRAFT_HISTORY_FILE: &str = "draft_history.jsonl";
pub(crate) const DRAFT_HISTORY_LIMIT: usize = 1_000;
/// Composer draft 历史使用的统一分片存储。
const SHARD_STORE: ShardedJsonlStore = ShardedJsonlStore::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryRecord {
    text: String,
}

pub(crate) fn draft_history_path(runtime_tmp_dir: &Path) -> PathBuf {
    runtime_tmp_dir
        .parent()
        .filter(|parent| parent.ends_with(USER_RUNTIME_DATA_DIR))
        .map_or_else(
            || runtime_tmp_dir.join(DRAFT_HISTORY_FILE),
            |data_dir| data_dir.join(USER_RUNTIME_SUBDIR).join(DRAFT_HISTORY_FILE),
        )
}

pub(crate) fn load_draft_history(path: &Path) -> Vec<String> {
    let mut entries = load_jsonl_history(path).unwrap_or_default();
    normalize_draft_history(&mut entries);
    entries
}

pub(crate) fn persist_draft_history(path: &Path, entries: &[String]) -> Result<(), io::Error> {
    let mut unique = entries.to_vec();
    normalize_draft_history(&mut unique);

    let lines = unique
        .into_iter()
        .map(|entry| {
            serde_json::to_vec(&HistoryRecord { text: entry })
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    SHARD_STORE.rewrite_with(path, |_| Ok(((), lines)))
}

fn load_jsonl_history(path: &Path) -> Result<Vec<String>, io::Error> {
    let lines = SHARD_STORE.read_lines(path)?;
    let entries = lines
        .iter()
        .filter_map(|line| serde_json::from_slice::<HistoryRecord>(line).ok())
        .map(|record| record.text)
        .collect::<Vec<_>>();
    Ok(entries)
}

fn normalize_draft_history(entries: &mut Vec<String>) {
    entries.retain(|entry| !entry.trim().is_empty());
    dedupe_latest_by_hash(entries);
    trim_to_limit(entries);
}

fn dedupe_latest_by_hash(entries: &mut Vec<String>) {
    let mut seen = HashMap::<u64, Vec<String>>::new();
    let mut output = Vec::with_capacity(entries.len());
    for entry in entries.drain(..).rev() {
        let hash = draft_history_hash(&entry);
        let bucket = seen.entry(hash).or_default();
        if bucket.iter().any(|stored| stored == &entry) {
            continue;
        }
        bucket.push(entry.clone());
        output.push(entry);
    }
    output.reverse();
    *entries = output;
}

fn draft_history_hash(entry: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    entry.hash(&mut hasher);
    hasher.finish()
}

fn trim_to_limit(entries: &mut Vec<String>) {
    let overflow = entries.len().saturating_sub(DRAFT_HISTORY_LIMIT);
    if overflow > 0 {
        entries.drain(..overflow);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DRAFT_HISTORY_LIMIT, SHARD_STORE, draft_history_path, load_draft_history,
        persist_draft_history,
    };

    #[test]
    fn history_path_uses_runtime_data_dir_next_to_tmp() {
        let axon_root = axon_test_support::test_temp_root().join("axon-runtime-root");
        let path = draft_history_path(&axon_root.join("data").join("tmp"));

        assert_eq!(
            path,
            axon_root
                .join("data")
                .join("runtime")
                .join("draft_history.jsonl")
        );
    }

    #[test]
    fn history_path_falls_back_inside_runtime_tmp_dir() {
        let runtime_tmp_dir = axon_test_support::test_temp_root().join("axon-custom-tmp");
        let path = draft_history_path(&runtime_tmp_dir);

        assert_eq!(path, runtime_tmp_dir.join("draft_history.jsonl"));
    }

    #[test]
    fn draft_history_uses_latest_entry_as_lru_position() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-tui-history-{}-{}.jsonl",
            std::process::id(),
            DRAFT_HISTORY_LIMIT
        ));
        let mut entries = (0..DRAFT_HISTORY_LIMIT + 2)
            .map(|index| format!("task-{index}"))
            .collect::<Vec<_>>();
        entries.push("task-0".to_owned());

        persist_draft_history(&path, &entries).expect("draft 历史应能原子写入");
        let loaded = load_draft_history(&path);

        assert_eq!(loaded.len(), DRAFT_HISTORY_LIMIT);
        assert_eq!(loaded.last().map(String::as_str), Some("task-0"));
        assert_eq!(loaded.iter().filter(|entry| *entry == "task-0").count(), 1);
        let _ = SHARD_STORE.remove_all(path);
    }

    #[test]
    fn history_store_preserves_multiline_entries() {
        let path = axon_test_support::test_temp_root().join(format!(
            "axon-tui-history-multiline-{}.jsonl",
            std::process::id()
        ));
        let entries = vec!["第一行\n第二行\n第三行".to_owned()];

        persist_draft_history(&path, &entries).expect("多行 draft 历史应能写入 JSONL");
        let loaded = load_draft_history(&path);

        assert_eq!(loaded, entries);
        let _ = SHARD_STORE.remove_all(path);
    }
}
