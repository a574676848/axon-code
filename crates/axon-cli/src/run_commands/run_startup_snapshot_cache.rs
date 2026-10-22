//! Daemon 长驻进程的启动快照缓存：后台预热 + TTL/指纹失效。
//!
//! Daemon 原先每轮 turn 都现场重建完整启动快照（Principal、Memory、User Agents、
//! Workspace 指令、Skills、MCP registry），这是首字延迟的主要来源。缓存把这份构建
//! 提前到进程启动后的后台线程，并在 turn 之间复用：TTL 到期后先用轻量指纹确认资源
//! 是否真的变化，指纹一致只续期，避免周期性重跑 MCP 探测。

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Instant,
};

use super::run_startup_snapshot::RunStartupSnapshot;
use super::run_startup_snapshot_fingerprint::startup_snapshot_fingerprint;

/// 快照在指纹未变化时的最长复用时间。
pub(crate) const STARTUP_SNAPSHOT_TTL_MS: u64 = 5 * 60 * 1_000;
/// 缓存条目上限；Daemon 同时服务的 Workspace 数量有限。
const STARTUP_SNAPSHOT_MAX_ENTRIES: usize = 4;

/// 进程级启动快照缓存；跨线程共享且可克隆。
#[derive(Clone)]
pub(crate) struct StartupSnapshotCache {
    entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
    building: Arc<BuildingSet>,
}

/// 正在构建的 key 集合；冷启动后立刻到来的 turn 据此等待同一次构建。
struct BuildingSet {
    keys: Mutex<HashSet<String>>,
    done: Condvar,
}

impl BuildingSet {
    fn new() -> Self {
        Self {
            keys: Mutex::new(HashSet::new()),
            done: Condvar::new(),
        }
    }
}

impl Default for StartupSnapshotCache {
    fn default() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            building: Arc::new(BuildingSet::new()),
        }
    }
}

#[derive(Clone)]
struct CacheEntry {
    snapshot: Arc<RunStartupSnapshot>,
    fingerprint: String,
    verified_at_ms: u64,
}

impl StartupSnapshotCache {
    /// 在后台线程预热首份快照。
    ///
    /// 预热失败只静默降级为按需重建，绝不改变 turn 的失败语义。
    pub(crate) fn spawn_prewarm(&self, config_path: PathBuf, cwd: PathBuf) {
        let cache = self.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let ready = cache.acquire(&config_path, &cwd).is_some();
            axon_logger::debug(&format!(
                "启动快照预热结束: ready={ready}, elapsed_ms={}",
                started.elapsed().as_millis()
            ));
        });
    }

    /// 取得本轮可复用的启动快照。
    ///
    /// 返回 `None` 表示本轮沿用未接入缓存前的行为（现场构建），调用方无需分支处理。
    pub(crate) fn acquire(
        &self,
        config_path: &Path,
        cwd: &Path,
    ) -> Option<Arc<RunStartupSnapshot>> {
        let key = cache_key(config_path, cwd);
        // 预热线程正在构建同一 key 时必须等待这次构建完成：Daemon 重启后积压的
        // turn 会立刻到达，重复一次全量构建只会让首字延迟翻倍。
        self.wait_for_build(&key);
        match self.read(&key) {
            Some(entry) if is_fresh(&entry, axon_time::unix_now_ms()) => Some(entry.snapshot),
            Some(entry) => {
                // TTL 到期：指纹未变通常只续期；但上一次构建存在探测失败的 MCP server 时
                // 必须重跑探测，否则恢复的 server 在本进程内永远不会重新出现。
                let reprobe_mcp = entry.snapshot.has_failed_mcp_servers();
                let fingerprint = startup_snapshot_fingerprint(config_path, cwd);
                if entry.fingerprint == fingerprint && !reprobe_mcp {
                    self.touch(&key);
                    return Some(entry.snapshot);
                }
                self.rebuild(config_path, cwd, key, fingerprint, reprobe_mcp)
            }
            None => {
                let fingerprint = startup_snapshot_fingerprint(config_path, cwd);
                self.rebuild(config_path, cwd, key, fingerprint, false)
            }
        }
    }

    /// 作废一个 Workspace 的快照条目；下一次 `acquire` 必定重建。
    pub(crate) fn invalidate(&self, config_path: &Path, cwd: &Path) {
        let key = cache_key(config_path, cwd);
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&key);
        }
    }

    /// 作废条目并在后台强制重建快照：MCP 探测绕过健康负缓存与 schema 缓存。
    ///
    /// 调用方是用户显式的刷新请求（TUI `/mcp`），因此这里不等待探测完成；
    /// 下一次 `acquire` 会等待这次构建结束后复用其结果。
    pub(crate) fn refresh_in_background(&self, config_path: PathBuf, cwd: PathBuf) {
        self.invalidate(&config_path, &cwd);
        let cache = self.clone();
        thread::spawn(move || {
            let _ = cache.force_rebuild(&config_path, &cwd);
        });
    }

    /// 强制重建快照并写回缓存；`refresh_mcp_health=true` 现场重探 MCP 能力。
    pub(crate) fn force_rebuild(
        &self,
        config_path: &Path,
        cwd: &Path,
    ) -> Option<Arc<RunStartupSnapshot>> {
        let key = cache_key(config_path, cwd);
        let fingerprint = startup_snapshot_fingerprint(config_path, cwd);
        let _guard = self.begin_build(&key);
        let snapshot =
            Arc::new(RunStartupSnapshot::build_with_mcp_refresh(config_path, cwd, true).ok()?);
        self.store(key, snapshot.clone(), fingerprint);
        Some(snapshot)
    }

    /// 重建快照：构建期间不持锁，MCP 探测耗时不能阻塞其他 lane 读取旧条目。
    fn rebuild(
        &self,
        config_path: &Path,
        cwd: &Path,
        key: String,
        fingerprint: String,
        force: bool,
    ) -> Option<Arc<RunStartupSnapshot>> {
        let _guard = self.begin_build(&key);
        // 其他 lane 可能刚刚完成同一 key 的构建，避免重复探测 MCP；
        // 强制重建（失败 MCP 重探）必须越过该短路。
        if !force
            && let Some(entry) = self.read(&key)
            && entry.fingerprint == fingerprint
        {
            self.touch(&key);
            return Some(entry.snapshot);
        }
        let snapshot = Arc::new(RunStartupSnapshot::build(config_path, cwd).ok()?);
        self.store(key, snapshot.clone(), fingerprint);
        Some(snapshot)
    }

    fn read(&self, key: &str) -> Option<CacheEntry> {
        let entries = self.entries.lock().ok()?;
        entries.get(key).cloned()
    }

    fn touch(&self, key: &str) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if let Some(entry) = entries.get_mut(key) {
            entry.verified_at_ms = axon_time::unix_now_ms();
        }
    }

    fn store(&self, key: String, snapshot: Arc<RunStartupSnapshot>, fingerprint: String) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if entries.len() >= STARTUP_SNAPSHOT_MAX_ENTRIES && !entries.contains_key(&key) {
            // 淘汰最旧的条目；Workspace 数量远小于上限，无需更复杂的淘汰策略。
            if let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, entry)| entry.verified_at_ms)
                .map(|(key, _)| key.clone())
            {
                entries.remove(&oldest);
            }
        }
        entries.insert(
            key,
            CacheEntry {
                snapshot,
                fingerprint,
                verified_at_ms: axon_time::unix_now_ms(),
            },
        );
    }

    /// 仅供测试：把条目推到 TTL 之外，验证过期后的指纹判定路径。
    #[cfg(test)]
    pub(crate) fn expire_for_tests(&self, config_path: &Path, cwd: &Path) {
        let key = cache_key(config_path, cwd);
        if let Ok(mut entries) = self.entries.lock()
            && let Some(entry) = entries.get_mut(&key)
        {
            entry.verified_at_ms = 0;
        }
    }

    /// 仅供测试：条目是否仍在缓存中。
    #[cfg(test)]
    pub(crate) fn contains_for_tests(&self, config_path: &Path, cwd: &Path) -> bool {
        self.read(&cache_key(config_path, cwd)).is_some()
    }

    fn begin_build(&self, key: &str) -> BuildGuard<'_> {
        if let Ok(mut keys) = self.building.keys.lock() {
            keys.insert(key.to_owned());
        }
        BuildGuard {
            cache: self,
            key: key.to_owned(),
        }
    }

    fn end_build(&self, key: &str) {
        if let Ok(mut keys) = self.building.keys.lock() {
            keys.remove(key);
        }
        self.building.done.notify_all();
    }

    fn wait_for_build(&self, key: &str) {
        let Ok(mut keys) = self.building.keys.lock() else {
            return;
        };
        while keys.contains(key) {
            match self.building.done.wait(keys) {
                Ok(guard) => keys = guard,
                Err(_) => return,
            }
        }
    }
}

/// TTL 判定：条目在 TTL 内无需扫描指纹。
fn is_fresh(entry: &CacheEntry, now_ms: u64) -> bool {
    now_ms.saturating_sub(entry.verified_at_ms) < STARTUP_SNAPSHOT_TTL_MS
}

/// 构建标记的 RAII 释放器：即使构建 panic 也会通知等待方。
struct BuildGuard<'a> {
    cache: &'a StartupSnapshotCache,
    key: String,
}

impl Drop for BuildGuard<'_> {
    fn drop(&mut self) {
        self.cache.end_build(&self.key);
    }
}

/// 缓存键使用规范化路径：Daemon 根目录与 Workspace 可能分别是 `C:\...` 与 `\\?\C:\...`。
fn cache_key(config_path: &Path, cwd: &Path) -> String {
    format!(
        "{}|{}",
        canonical_or_original(config_path).display(),
        canonical_or_original(cwd).display()
    )
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::StartupSnapshotCache;
    use std::{
        fs,
        path::PathBuf,
        sync::Arc,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn ttl_expired_entry_renews_without_rebuild_when_fingerprint_is_unchanged() {
        let (root, config_path, workspace) = snapshot_fixture("snapshot-renew");
        let cache = StartupSnapshotCache::default();

        let first = cache
            .acquire(&config_path, &workspace)
            .expect("首次应构建快照");
        cache.expire_for_tests(&config_path, &workspace);
        let second = cache
            .acquire(&config_path, &workspace)
            .expect("TTL 过期后应续期");

        assert!(
            Arc::ptr_eq(&first, &second),
            "指纹未变时只续期，不得重建快照"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn invalidate_then_force_rebuild_replaces_cached_snapshot() {
        let (root, config_path, workspace) = snapshot_fixture("snapshot-invalidate");
        let cache = StartupSnapshotCache::default();
        let first = cache
            .acquire(&config_path, &workspace)
            .expect("首次应构建快照");
        assert!(cache.contains_for_tests(&config_path, &workspace));

        cache.invalidate(&config_path, &workspace);

        assert!(
            !cache.contains_for_tests(&config_path, &workspace),
            "作废后条目必须消失，下一次 acquire 必定重建"
        );
        let rebuilt = cache
            .force_rebuild(&config_path, &workspace)
            .expect("强制重建应成功");
        assert!(!Arc::ptr_eq(&first, &rebuilt), "强制重建必须产生新快照");
        assert!(cache.contains_for_tests(&config_path, &workspace));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn ttl_expired_entry_rebuilds_when_workspace_instruction_changes() {
        let (root, config_path, workspace) = snapshot_fixture("snapshot-rebuild");
        let cache = StartupSnapshotCache::default();

        let first = cache
            .acquire(&config_path, &workspace)
            .expect("首次应构建快照");
        let changed = "changed-workspace-rule";
        fs::write(workspace.join("AGENTS.md"), changed).expect("写入工作区指令");
        cache.expire_for_tests(&config_path, &workspace);
        let second = cache
            .acquire(&config_path, &workspace)
            .expect("指纹变化后应重建");

        assert!(!Arc::ptr_eq(&first, &second), "指纹变化必须重建快照");
        assert!(
            second
                .workspace_instructions
                .iter()
                .any(|instruction| instruction.content.contains(changed)),
            "重建后的快照必须包含新的工作区指令"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn wait_for_build_blocks_until_the_inflight_build_finishes() {
        let cache = StartupSnapshotCache::default();
        let guard = cache.begin_build("key-under-test");
        let wait_cache = cache.clone();
        let waiter = std::thread::spawn(move || wait_cache.wait_for_build("key-under-test"));

        std::thread::sleep(std::time::Duration::from_millis(80));
        let released_before_marker_drop = waiter.is_finished();
        drop(guard);
        waiter.join().expect("等待线程应正常结束");

        assert!(
            !released_before_marker_drop,
            "构建标记未释放前，等待方不得提前返回"
        );
    }

    fn snapshot_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = temp_root(label);
        let workspace = root.join("workspace");
        fs::create_dir_all(&workspace).expect("创建工作区");
        let config_path = root.join("runtime").join("config.toml");
        fs::create_dir_all(config_path.parent().expect("配置目录")).expect("创建配置目录");
        axon_config::ConfigLoader::save_to_path(&config_path, &axon_config::AxonConfig::default())
            .expect("写入配置");
        (root, config_path, workspace)
    }

    fn temp_root(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root()
            .join(format!("axon-cli-{label}-{}-{nonce}", std::process::id()))
    }
}
