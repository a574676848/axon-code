use std::{
    collections::BTreeMap,
    fs,
    sync::atomic::{AtomicBool, Ordering},
};

use super::{
    model::{
        FileFingerprint, INDEX_FORMAT_VERSION, IndexKey, IndexSnapshot, IndexedFile,
        SEARCH_INDEX_SCHEMA_VERSION, SearchIndexPolicy, TrigramKey, relative_path,
    },
    trigram::extract,
};
use crate::workspace_walker::WorkspaceWalkEntry;

const SEARCH_INDEX_BINARY_BYTE: u8 = 0;

/// 单次文件读取产出的索引数据；全量构建与增量覆盖层共用同一套判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileIndexData {
    pub(crate) fingerprint: FileFingerprint,
    pub(crate) searchable: bool,
    pub(crate) trigrams: Vec<TrigramKey>,
    /// 本次判定实际读取的字节数，用于累计构建预算。
    pub(crate) bytes_read: u64,
}

impl FileIndexData {
    fn unsearchable(fingerprint: FileFingerprint, bytes_read: u64) -> Self {
        Self {
            fingerprint,
            searchable: false,
            trigrams: Vec::new(),
            bytes_read,
        }
    }
}

/// 读取单个文件并产出索引数据。
///
/// 元数据不可读、超过单文件上限、超过剩余预算、二进制、非 UTF-8 或读前读后指纹不一致时，
/// 一律标记为不可优化，由精确搜索继续扫描，保证候选过滤不会漏检。
pub(crate) fn index_file(
    entry: &WorkspaceWalkEntry,
    max_file_size: u64,
    remaining_bytes: u64,
) -> FileIndexData {
    let fallback = FileFingerprint {
        length: entry.file_len,
        modified_at_ms: entry.modified_at_ms,
    };
    let Some(metadata) = fs::metadata(&entry.path).ok() else {
        return FileIndexData::unsearchable(fallback, 0);
    };
    let fingerprint = FileFingerprint::from_metadata(&metadata).unwrap_or(fallback);
    if metadata.len() > max_file_size || metadata.len() > remaining_bytes {
        return FileIndexData::unsearchable(fingerprint, 0);
    }

    let Ok(bytes) = fs::read(&entry.path) else {
        return FileIndexData::unsearchable(fingerprint, 0);
    };
    let bytes_read = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if bytes_read > remaining_bytes {
        return FileIndexData::unsearchable(fingerprint, bytes_read);
    }

    let stable = fs::metadata(&entry.path)
        .ok()
        .and_then(|current| FileFingerprint::from_metadata(&current))
        .is_some_and(|current| current == fingerprint);
    if !stable {
        return FileIndexData::unsearchable(fingerprint, bytes_read);
    }

    let searchable =
        !bytes.contains(&SEARCH_INDEX_BINARY_BYTE) && std::str::from_utf8(&bytes).is_ok();
    let trigrams = if searchable {
        extract(&bytes)
    } else {
        Vec::new()
    };
    FileIndexData {
        fingerprint,
        searchable,
        trigrams,
        bytes_read,
    }
}

/// 按 walker 提供的文件全集构建新 generation。
///
/// 取消发生时返回 `None`，绝不发布部分索引。
pub(crate) fn build_snapshot(
    key: &IndexKey,
    entries: &[WorkspaceWalkEntry],
    generation: u64,
    policy: SearchIndexPolicy,
    cancel: &AtomicBool,
) -> Option<IndexSnapshot> {
    let mut remaining_indexed_bytes = policy.max_indexed_bytes;
    let mut collected: Vec<(String, FileIndexData)> = Vec::new();

    for entry in entries.iter().filter(|entry| !entry.is_dir) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let Some(relative) = relative_path(&key.root, &entry.path) else {
            continue;
        };
        let data = index_file(entry, policy.max_file_size, remaining_indexed_bytes);
        remaining_indexed_bytes = remaining_indexed_bytes.saturating_sub(data.bytes_read);
        collected.push((relative, data));
    }

    // 按相对路径排序分配 id，保证同一文件集在任意轮次得到相同编号。
    collected.sort_by(|left, right| left.0.cmp(&right.0));

    let mut files = BTreeMap::new();
    let mut postings = BTreeMap::<TrigramKey, Vec<u32>>::new();
    for (index, (relative, data)) in collected.into_iter().enumerate() {
        let Ok(id) = u32::try_from(index) else {
            break;
        };
        if data.searchable {
            for trigram in &data.trigrams {
                postings.entry(*trigram).or_default().push(id);
            }
        }
        files.insert(
            relative,
            IndexedFile {
                id,
                fingerprint: data.fingerprint,
                searchable: data.searchable,
            },
        );
    }

    for ids in postings.values_mut() {
        ids.sort_unstable();
        ids.dedup();
    }

    let mut snapshot = IndexSnapshot {
        format_version: INDEX_FORMAT_VERSION,
        schema_version: SEARCH_INDEX_SCHEMA_VERSION,
        root: key.root.clone(),
        hidden: key.hidden,
        gitignore: key.gitignore,
        generation,
        files,
        postings,
        paths: BTreeMap::new(),
    };
    snapshot.rebuild_paths();
    Some(snapshot)
}
