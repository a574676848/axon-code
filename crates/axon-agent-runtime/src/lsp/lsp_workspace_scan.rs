//! LSP 工作区文件扫描与覆盖汇总。

use std::{
    collections::{BTreeSet, HashMap},
    path::Path,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

use axon_fs_walk::{Walk, WalkAction, WalkOpts};
use serde::Serialize;

use crate::{
    LspCatalogRuntime, LspWorkspaceMatch,
    runtime_limits::{
        LSP_WORKSPACE_COVERAGE_CACHE_MAX_ENTRIES, LSP_WORKSPACE_COVERAGE_CACHE_TTL_SECS,
    },
};

const WORKSPACE_COVERAGE_CACHE_TTL: Duration =
    Duration::from_secs(LSP_WORKSPACE_COVERAGE_CACHE_TTL_SECS);

fn coverage_cache() -> &'static Mutex<WorkspaceCoverageCache> {
    static CACHE: OnceLock<Mutex<WorkspaceCoverageCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(WorkspaceCoverageCache::default()))
}

#[derive(Default)]
struct WorkspaceCoverageCache {
    entries: HashMap<String, WorkspaceCoverageCacheEntry>,
    access_clock: u64,
}

struct WorkspaceCoverageCacheEntry {
    coverage: LspWorkspaceCoverage,
    cached_at: Instant,
    last_access: u64,
}

impl WorkspaceCoverageCache {
    fn get(&mut self, key: &str, now: Instant) -> Option<LspWorkspaceCoverage> {
        let entry = self.entries.get_mut(key)?;
        if now.duration_since(entry.cached_at) > WORKSPACE_COVERAGE_CACHE_TTL {
            self.entries.remove(key);
            return None;
        }
        self.access_clock = self.access_clock.saturating_add(1);
        entry.last_access = self.access_clock;
        Some(entry.coverage.clone())
    }

    fn insert(&mut self, key: String, coverage: LspWorkspaceCoverage, now: Instant) {
        self.access_clock = self.access_clock.saturating_add(1);
        if self.entries.len() >= LSP_WORKSPACE_COVERAGE_CACHE_MAX_ENTRIES
            && !self.entries.contains_key(&key)
            && let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_access)
                .map(|(key, _)| key.clone())
        {
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            WorkspaceCoverageCacheEntry {
                coverage,
                cached_at: now,
                last_access: self.access_clock,
            },
        );
    }
}

/// 工作区级 LSP 覆盖汇总。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LspWorkspaceCoverage {
    /// 工作区根目录。
    pub workspace_root: String,
    /// 实际扫描的候选文件数。
    pub scanned_files: usize,
    /// 是否因显式调用方上限或扫描错误而截断。
    pub truncated: bool,
    /// 当前工作区命中的 server。
    pub matched_servers: Vec<String>,
    /// 因 root marker 缺失而被过滤掉的 server。
    pub filtered_servers: Vec<String>,
    /// 采样的文件命中明细。
    pub file_matches: Vec<LspWorkspaceMatch>,
}

impl LspCatalogRuntime {
    /// 在工作区内扫描候选文件并汇总 LSP 覆盖情况。
    #[must_use]
    pub fn resolve_workspace_coverage(workspace_root: &Path) -> LspWorkspaceCoverage {
        let key = stable_workspace_key(workspace_root);
        let now = Instant::now();
        if let Some(coverage) = coverage_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key, now)
        {
            return coverage;
        }
        let coverage = Self::resolve_workspace_coverage_with_limit(workspace_root, usize::MAX);
        coverage_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, coverage.clone(), now);
        coverage
    }

    /// 在 Axon 写入工作区文件后使覆盖缓存立即失效。
    pub fn invalidate_workspace_coverage(workspace_root: &Path) {
        let key = stable_workspace_key(workspace_root);
        coverage_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .remove(&key);
    }

    /// 在工作区内扫描候选文件并汇总 LSP 覆盖情况，可显式限制扫描文件数。
    #[must_use]
    pub fn resolve_workspace_coverage_with_limit(
        workspace_root: &Path,
        max_scanned_files: usize,
    ) -> LspWorkspaceCoverage {
        let mut scanned_files = 0usize;
        let mut truncated = false;
        let mut matched_servers = BTreeSet::new();
        let mut filtered_servers = BTreeSet::new();
        let mut file_matches = Vec::new();
        let mut matched_keys = BTreeSet::new();
        let opts = WalkOpts {
            require_file: true,
            ..WalkOpts::default()
        };
        let walk_truncated = match Walk::run(workspace_root, &opts, |entry| {
            let Some(matched) = Self::resolve_workspace_match(workspace_root, &entry.path) else {
                return WalkAction::Continue;
            };
            let key = matched.extension.clone();
            scanned_files = scanned_files.saturating_add(1);
            if let Some(limit) = (max_scanned_files != usize::MAX).then_some(max_scanned_files)
                && scanned_files >= limit
            {
                truncated = true;
            }
            matched_servers.extend(matched.matched_servers.iter().cloned());
            filtered_servers.extend(matched.filtered_by_root_markers.iter().cloned());
            if matched_keys.insert(key) {
                file_matches.push(matched);
            }
            if truncated {
                WalkAction::Stop
            } else {
                WalkAction::Continue
            }
        }) {
            Ok(summary) => summary.truncated,
            Err(_) => {
                truncated = true;
                true
            }
        };
        truncated |= walk_truncated;
        LspWorkspaceCoverage {
            workspace_root: workspace_root.display().to_string(),
            scanned_files,
            truncated,
            matched_servers: matched_servers.into_iter().collect(),
            filtered_servers: filtered_servers.into_iter().collect(),
            file_matches,
        }
    }
}

fn stable_workspace_key(workspace_root: &Path) -> String {
    workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_coverage_collects_multi_language_matches() {
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-workspace-coverage");
        let rust_dir = workspace.join("rust");
        let ts_dir = workspace.join("web");
        std::fs::create_dir_all(&rust_dir).unwrap();
        std::fs::create_dir_all(&ts_dir).unwrap();
        std::fs::write(rust_dir.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        std::fs::write(rust_dir.join("lib.rs"), "pub fn demo() {}\n").unwrap();
        std::fs::write(ts_dir.join("package-lock.json"), "{}").unwrap();
        std::fs::write(ts_dir.join("index.ts"), "export const demo = 1;\n").unwrap();

        let coverage = LspCatalogRuntime::resolve_workspace_coverage_with_limit(&workspace, 32);

        assert!(
            coverage
                .matched_servers
                .contains(&"rust-analyzer".to_owned())
        );
        assert!(coverage.matched_servers.contains(&"typescript".to_owned()));
        assert!(!coverage.truncated);
        assert!(coverage.scanned_files >= 2);
    }

    #[test]
    fn workspace_coverage_cache_reuses_until_explicit_invalidation() {
        let workspace = axon_test_support::test_temp_root()
            .join(format!("axon-lsp-workspace-cache-{}", std::process::id()));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        std::fs::write(workspace.join("lib.rs"), "pub fn demo() {}\n").unwrap();

        let first = LspCatalogRuntime::resolve_workspace_coverage(&workspace);
        std::fs::remove_file(workspace.join("lib.rs")).unwrap();
        std::fs::remove_file(workspace.join("Cargo.toml")).unwrap();
        let cached = LspCatalogRuntime::resolve_workspace_coverage(&workspace);
        LspCatalogRuntime::invalidate_workspace_coverage(&workspace);
        let refreshed = LspCatalogRuntime::resolve_workspace_coverage(&workspace);

        assert!(first.matched_servers.contains(&"rust-analyzer".to_owned()));
        assert_eq!(cached, first);
        assert!(
            !refreshed
                .matched_servers
                .contains(&"rust-analyzer".to_owned())
        );
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn workspace_coverage_scans_beyond_legacy_file_limit() {
        let workspace = axon_test_support::test_temp_root().join(format!(
            "axon-lsp-workspace-unbounded-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("Cargo.toml"), "[package]\nname='demo'\n").unwrap();
        for index in 0..2_001 {
            std::fs::write(workspace.join(format!("module_{index}.rs")), "").unwrap();
        }

        let coverage = LspCatalogRuntime::resolve_workspace_coverage(&workspace);

        assert_eq!(coverage.scanned_files, 2_001);
        assert!(!coverage.truncated);
        assert!(
            coverage
                .matched_servers
                .contains(&"rust-analyzer".to_owned())
        );
        assert_eq!(coverage.file_matches.len(), 1);

        let _ = std::fs::remove_dir_all(workspace);
    }
}
