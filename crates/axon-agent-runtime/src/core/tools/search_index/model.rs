use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

pub(crate) const INDEX_FORMAT_VERSION: u32 = 1;
pub(crate) const SEARCH_INDEX_DIRECTORY: &str = "search-index";
pub(crate) const SEARCH_INDEX_FILE: &str = "index.json";
pub(crate) const SEARCH_INDEX_MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;
pub(crate) const SEARCH_INDEX_MAX_INDEXED_BYTES: u64 = 256 * 1024 * 1024;
pub(crate) const SEARCH_INDEX_MIN_WORKSPACE_FILES: usize = 16;
/// 同一 workspace 两次后台构建之间的最小间隔，避免编辑期间反复全量重建。
pub(crate) const SEARCH_INDEX_REBUILD_COOLDOWN: Duration = Duration::from_secs(30);
/// 同时保留在内存中的 workspace 索引状态上限，超出后按最近使用淘汰。
pub(crate) const SEARCH_INDEX_MAX_TRACKED_WORKSPACES: usize = 4;
/// 单个 workspace 允许常驻的覆盖层条目数上限，达到后合并进基础快照。
pub(crate) const SEARCH_INDEX_MAX_OVERLAY_FILES: usize = 256;
/// 单个 workspace 允许常驻的覆盖层字节上限，达到后合并进基础快照。
pub(crate) const SEARCH_INDEX_MAX_OVERLAY_BYTES: u64 = 32 * 1024 * 1024;
/// 候选语义版本；posting 由路径字符串改为文件 id 后升为 3。
pub(crate) const SEARCH_INDEX_SCHEMA_VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct TrigramKey(pub(crate) u32);

/// 索引身份：根路径与 walker 可见性开关共同决定一份快照归属哪个 workspace 视图。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct IndexKey {
    pub(crate) root: PathBuf,
    pub(crate) hidden: bool,
    pub(crate) gitignore: bool,
}

impl IndexKey {
    pub(crate) fn new(root: &Path, hidden: bool, gitignore: bool) -> Self {
        Self {
            root: normalize_index_root(root),
            hidden,
            gitignore,
        }
    }
}

/// 统一工作区根路径的物理形态，作为索引身份的唯一来源。
///
/// 同一目录可能以 8.3 短名（例如 CI 的 `RUNNER~1`）、`\\?\` 扩展前缀或普通绝对
/// 路径传入。若直接把调用方写法当作缓存键，同一份快照会因写法不同而互不命中，
/// 表现为索引已构建却始终退化为完整扫描。这里统一解析为真实路径，并在 Windows
/// 上剥离扩展前缀，使键与调用方传入的普通路径保持可比。
fn normalize_index_root(root: &Path) -> PathBuf {
    let Ok(canonical) = std::fs::canonicalize(root) else {
        return root.to_path_buf();
    };
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    canonical
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileFingerprint {
    pub(crate) length: u64,
    pub(crate) modified_at_ms: u128,
}

impl FileFingerprint {
    pub(crate) fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        let modified = metadata.modified().ok()?;
        let modified_at_ms = modified
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()?
            .as_millis();
        Some(Self {
            length: metadata.len(),
            modified_at_ms,
        })
    }
}

/// 单个已索引文件的元数据；`id` 是 posting 中引用的稳定编号。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IndexedFile {
    pub(crate) id: u32,
    pub(crate) fingerprint: FileFingerprint,
    pub(crate) searchable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IndexSnapshot {
    pub(crate) format_version: u32,
    pub(crate) schema_version: u32,
    pub(crate) root: PathBuf,
    pub(crate) hidden: bool,
    pub(crate) gitignore: bool,
    pub(crate) generation: u64,
    /// 相对路径 → 文件元数据。
    pub(crate) files: BTreeMap<String, IndexedFile>,
    /// trigram → 升序去重的文件 id 列表。
    pub(crate) postings: BTreeMap<TrigramKey, Vec<u32>>,
    /// 派生表 `id → 相对路径`；不进入持久化格式，构建或载入后由 `rebuild_paths` 重建。
    #[serde(skip)]
    pub(crate) paths: BTreeMap<u32, String>,
}

impl IndexSnapshot {
    /// 在构建或载入后重建派生路径表，保证 posting 的 id 可反查相对路径。
    pub(crate) fn rebuild_paths(&mut self) {
        self.paths = self
            .files
            .iter()
            .map(|(path, file)| (file.id, path.clone()))
            .collect();
    }

    pub(crate) fn path_of(&self, id: u32) -> Option<&str> {
        self.paths.get(&id).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SearchIndexPolicy {
    pub(crate) max_file_size: u64,
    pub(crate) max_indexed_bytes: u64,
    pub(crate) min_workspace_files: usize,
    pub(crate) rebuild_cooldown: Duration,
    pub(crate) max_tracked_workspaces: usize,
    pub(crate) max_overlay_files: usize,
    pub(crate) max_overlay_bytes: u64,
}

impl Default for SearchIndexPolicy {
    fn default() -> Self {
        Self {
            max_file_size: SEARCH_INDEX_MAX_FILE_SIZE,
            max_indexed_bytes: SEARCH_INDEX_MAX_INDEXED_BYTES,
            min_workspace_files: SEARCH_INDEX_MIN_WORKSPACE_FILES,
            rebuild_cooldown: SEARCH_INDEX_REBUILD_COOLDOWN,
            max_tracked_workspaces: SEARCH_INDEX_MAX_TRACKED_WORKSPACES,
            max_overlay_files: SEARCH_INDEX_MAX_OVERLAY_FILES,
            max_overlay_bytes: SEARCH_INDEX_MAX_OVERLAY_BYTES,
        }
    }
}

impl SearchIndexPolicy {
    pub(crate) fn should_build(&self, file_count: usize) -> bool {
        file_count >= self.min_workspace_files
    }
}

pub(crate) fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let text = relative.to_string_lossy();
    if text.contains('\\') {
        Some(text.replace('\\', "/"))
    } else {
        Some(text.into_owned())
    }
}

pub(crate) fn validate_snapshot(snapshot: &IndexSnapshot, key: &IndexKey) -> bool {
    if snapshot.format_version != INDEX_FORMAT_VERSION
        || snapshot.schema_version != SEARCH_INDEX_SCHEMA_VERSION
        || snapshot.root != key.root
        || snapshot.hidden != key.hidden
        || snapshot.gitignore != key.gitignore
        || snapshot.generation == 0
    {
        return false;
    }

    let mut known_ids = BTreeSet::new();
    if !snapshot
        .files
        .values()
        .all(|file| known_ids.insert(file.id))
    {
        return false;
    }

    snapshot.postings.values().all(|ids| {
        ids.windows(2).all(|window| window[0] < window[1])
            && ids.iter().all(|id| known_ids.contains(id))
    })
}
