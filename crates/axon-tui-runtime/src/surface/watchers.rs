//! 外部 watcher 的统一事件模型。

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const CATALOG_REFRESH_WATCHER_INTERVAL: Duration = Duration::from_secs(3);
const MCP_CONFIG_FILE_NAME: &str = ".mcp.json";
const AXON_DIRECTORY_NAME: &str = ".axon";
const SKILLS_DIRECTORY_NAME: &str = "skills";
const SKILL_FILE_NAME: &str = "SKILL.md";

/// 外部 watcher 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeWatcherEvent {
    /// 事件来源。
    pub source: RuntimeWatcherSource,
    /// watcher 当前健康状态。
    pub status: RuntimeWatcherStatus,
    /// watcher 载荷。
    pub payload: RuntimeWatcherPayload,
}

impl RuntimeWatcherEvent {
    fn new(
        source: RuntimeWatcherSource,
        status: RuntimeWatcherStatus,
        payload: RuntimeWatcherPayload,
    ) -> Self {
        Self {
            source,
            status,
            payload,
        }
    }

    /// 创建 MCP 主动刷新事件。
    #[must_use]
    pub fn mcp_refresh() -> Self {
        Self::refresh(RuntimeWatcherSource::Mcp)
    }

    /// 创建 Skills 主动刷新事件。
    #[must_use]
    pub fn skills_refresh() -> Self {
        Self::refresh(RuntimeWatcherSource::Skills)
    }

    fn refresh(source: RuntimeWatcherSource) -> Self {
        Self::new(
            source,
            RuntimeWatcherStatus::Ready,
            RuntimeWatcherPayload::Refresh(RuntimeWatcherRefreshPayload { source }),
        )
    }
}

/// watcher 来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RuntimeWatcherSource {
    /// MCP catalog 主动刷新。
    Mcp,
    /// Skills catalog 主动刷新。
    Skills,
}

impl RuntimeWatcherSource {
    /// 稳定来源编码。
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Mcp => "mcp",
            Self::Skills => "skills",
        }
    }
}

/// watcher 健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeWatcherStatus {
    /// 正常观测。
    Ready,
    /// 观测降级。
    Degraded,
    /// 观测失败。
    Failed,
    /// 当前来源没有启用 watcher。
    Disabled,
}

/// watcher 载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeWatcherPayload {
    /// 主动刷新请求。
    Refresh(RuntimeWatcherRefreshPayload),
}

/// watcher 主动刷新载荷。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeWatcherRefreshPayload {
    /// 需要刷新的 catalog 来源。
    pub source: RuntimeWatcherSource,
}

/// watcher registry，统一管理外部观测的节流与变更指纹。
#[derive(Debug, Clone)]
pub struct RuntimeWatcherRegistry {
    next_catalog_refresh: Instant,
    last_mcp_fingerprint: Option<CatalogFingerprint>,
    last_skills_fingerprint: Option<CatalogFingerprint>,
}

impl RuntimeWatcherRegistry {
    /// 创建 watcher registry。
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            next_catalog_refresh: now + CATALOG_REFRESH_WATCHER_INTERVAL,
            last_mcp_fingerprint: None,
            last_skills_fingerprint: None,
        }
    }

    /// 收集到期 watcher 事件。
    pub fn drain_due_events(
        &mut self,
        now: Instant,
        config_path: Option<&Path>,
        cwd: &Path,
    ) -> Vec<RuntimeWatcherEvent> {
        self.drain_catalog_refresh_events(now, config_path, cwd)
    }

    fn drain_catalog_refresh_events(
        &mut self,
        now: Instant,
        config_path: Option<&Path>,
        cwd: &Path,
    ) -> Vec<RuntimeWatcherEvent> {
        if now < self.next_catalog_refresh {
            return Vec::new();
        }
        self.next_catalog_refresh = now + CATALOG_REFRESH_WATCHER_INTERVAL;

        let mcp_fingerprint = mcp_catalog_fingerprint(config_path, cwd);
        let skills_fingerprint = skills_catalog_fingerprint(cwd);
        let mcp_changed = self
            .last_mcp_fingerprint
            .replace(mcp_fingerprint)
            .is_some_and(|previous| previous != mcp_fingerprint);
        let skills_changed = self
            .last_skills_fingerprint
            .replace(skills_fingerprint)
            .is_some_and(|previous| previous != skills_fingerprint);

        let mut events = Vec::new();
        if mcp_changed {
            events.push(RuntimeWatcherEvent::mcp_refresh());
        }
        if skills_changed {
            events.push(RuntimeWatcherEvent::skills_refresh());
        }
        events
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CatalogFingerprint(u64);

fn mcp_catalog_fingerprint(config_path: Option<&Path>, cwd: &Path) -> CatalogFingerprint {
    let mut paths = vec![cwd.join(MCP_CONFIG_FILE_NAME)];
    let user_root = config_path
        .and_then(|path| axon_config::runtime_axon_root_path(path).ok())
        .or_else(|| {
            axon_config::user_home_dir()
                .ok()
                .map(|home| home.join(AXON_DIRECTORY_NAME))
        });
    if let Some(root) = user_root {
        paths.push(root.join(MCP_CONFIG_FILE_NAME));
    }
    paths.sort();
    paths.dedup();
    fingerprint_files(&paths)
}

fn skills_catalog_fingerprint(cwd: &Path) -> CatalogFingerprint {
    let mut roots = vec![cwd.join(AXON_DIRECTORY_NAME).join(SKILLS_DIRECTORY_NAME)];
    if let Ok(home) = axon_config::user_home_dir() {
        roots.push(home.join(AXON_DIRECTORY_NAME).join(SKILLS_DIRECTORY_NAME));
    }
    roots.sort();
    roots.dedup();

    let mut hasher = DefaultHasher::new();
    for root in roots {
        root.hash(&mut hasher);
        match fs::read_dir(&root) {
            Ok(entries) => {
                let mut paths = entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>();
                paths.sort();
                for path in paths {
                    path.hash(&mut hasher);
                    if path.is_dir() {
                        let skill_file = path.join(SKILL_FILE_NAME);
                        skill_file.hash(&mut hasher);
                        hash_file(&skill_file, &mut hasher);
                    }
                }
            }
            Err(error) => error.kind().hash(&mut hasher),
        }
    }
    CatalogFingerprint(hasher.finish())
}

fn fingerprint_files(paths: &[PathBuf]) -> CatalogFingerprint {
    let mut hasher = DefaultHasher::new();
    for path in paths {
        path.hash(&mut hasher);
        hash_file(path, &mut hasher);
    }
    CatalogFingerprint(hasher.finish())
}

fn hash_file(path: &Path, hasher: &mut DefaultHasher) {
    match fs::read(path) {
        Ok(contents) => {
            true.hash(hasher);
            contents.hash(hasher);
        }
        Err(error) => {
            false.hash(hasher);
            error.kind().hash(hasher);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn catalog_refresh_watchers_emit_only_after_catalog_changes() {
        let temp = axon_test_support::test_temp_dir();
        let cwd = temp.path().join("workspace");
        fs::create_dir_all(&cwd).expect("工作区目录应可创建");
        let config_path = temp.path().join("config.toml");
        let mut registry = RuntimeWatcherRegistry::new(Instant::now());
        let now = Instant::now();

        let initial = registry.drain_due_events(
            now + CATALOG_REFRESH_WATCHER_INTERVAL,
            Some(&config_path),
            &cwd,
        );
        assert!(initial.iter().all(|event| !matches!(
            event.source,
            RuntimeWatcherSource::Mcp | RuntimeWatcherSource::Skills
        )));

        fs::write(cwd.join(MCP_CONFIG_FILE_NAME), "{}").expect("MCP 配置应可写入");
        let skill_dir = cwd
            .join(AXON_DIRECTORY_NAME)
            .join(SKILLS_DIRECTORY_NAME)
            .join("demo");
        fs::create_dir_all(&skill_dir).expect("Skills 目录应可创建");
        fs::write(
            skill_dir.join(SKILL_FILE_NAME),
            "---
name: demo
description: demo
---
",
        )
        .expect("Skill 文件应可写入");

        let changed = registry.drain_due_events(
            now + CATALOG_REFRESH_WATCHER_INTERVAL.saturating_mul(2),
            Some(&config_path),
            &cwd,
        );
        assert!(
            changed
                .iter()
                .any(|event| event.source == RuntimeWatcherSource::Mcp)
        );
        assert!(
            changed
                .iter()
                .any(|event| event.source == RuntimeWatcherSource::Skills)
        );

        let unchanged = registry.drain_due_events(
            now + CATALOG_REFRESH_WATCHER_INTERVAL.saturating_mul(3),
            Some(&config_path),
            &cwd,
        );
        assert!(unchanged.iter().all(|event| !matches!(
            event.source,
            RuntimeWatcherSource::Mcp | RuntimeWatcherSource::Skills
        )));
    }
}
