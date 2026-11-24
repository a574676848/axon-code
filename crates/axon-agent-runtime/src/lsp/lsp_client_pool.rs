//! LSP client 复用池。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use axon_domain::{AxonError, AxonResult};

use crate::lsp_document::path_to_file_uri;
use crate::runtime_limits::{LSP_CLIENT_BROKEN_BACKOFF_SECS, LSP_CLIENT_IDLE_TIMEOUT_SECS};
use crate::{LspClientInitializeInput, LspLaunchPlan, LspStdioClient};

const LSP_CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(LSP_CLIENT_IDLE_TIMEOUT_SECS);
const LSP_CLIENT_BROKEN_BACKOFF: Duration = Duration::from_secs(LSP_CLIENT_BROKEN_BACKOFF_SECS);
const LSP_CLIENT_LOCK_RETRY: Duration = Duration::from_millis(10);
const LSP_CLIENT_DEADLINE_EXCEEDED: &str = "LSP client 获取或初始化超过诊断窗口";

fn pool() -> &'static Mutex<LspClientPoolState> {
    static POOL: OnceLock<Mutex<LspClientPoolState>> = OnceLock::new();
    POOL.get_or_init(|| Mutex::new(LspClientPoolState::default()))
}

/// LSP client pool 统计快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LspClientPoolStats {
    /// 当前可复用 client 数。
    pub active_clients: usize,
    /// 累计启动次数。
    pub starts: u64,
    /// 累计复用次数。
    pub reuses: u64,
    /// 启动去重命中次数。
    pub spawn_coalesces: u64,
    /// 命中 broken backoff 的次数。
    pub broken_backoffs: u64,
    /// 因失败标记 broken 的次数。
    pub broken_marks: u64,
    /// shutdown / idle 回收次数。
    pub shutdowns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct LspClientPoolKey {
    workspace_root: String,
    server_id: String,
}

struct LspClientPoolEntry {
    client: Option<LspStdioClient>,
    last_used_at: Instant,
    broken_until: Option<Instant>,
}

impl LspClientPoolEntry {
    fn new(now: Instant) -> Self {
        Self {
            client: None,
            last_used_at: now,
            broken_until: None,
        }
    }

    fn shutdown(&mut self) -> bool {
        let Some(mut client) = self.client.take() else {
            return false;
        };
        client.shutdown();
        true
    }
}

#[derive(Default)]
struct LspClientPoolState {
    clients: HashMap<LspClientPoolKey, Arc<Mutex<LspClientPoolEntry>>>,
    stats: LspClientPoolStats,
}

impl LspClientPoolState {
    fn evict_idle(&mut self, now: Instant) {
        let mut expired = Vec::new();
        for (key, slot) in &self.clients {
            let remove = match slot.try_lock() {
                Ok(mut entry) => {
                    let idle = entry.client.is_some()
                        && now.duration_since(entry.last_used_at) >= LSP_CLIENT_IDLE_TIMEOUT;
                    if idle && entry.shutdown() {
                        self.stats.shutdowns = self.stats.shutdowns.saturating_add(1);
                    }
                    idle || (entry.client.is_none()
                        && entry.broken_until.is_none_or(|until| until <= now))
                }
                Err(TryLockError::Poisoned(poisoned)) => {
                    let mut entry = poisoned.into_inner();
                    if entry.shutdown() {
                        self.stats.shutdowns = self.stats.shutdowns.saturating_add(1);
                    }
                    true
                }
                Err(TryLockError::WouldBlock) => false,
            };
            if remove {
                expired.push(key.clone());
            }
        }
        for key in expired {
            self.clients.remove(&key);
        }
        self.stats.active_clients = active_client_count(&self.clients);
    }
}

/// LSP client pool runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct LspClientPoolRuntime;

impl LspClientPoolRuntime {
    /// 复用或启动指定 LSP client；同一 workspace/server 串行，不同 slot 可并行。
    ///
    pub fn with_client<T>(
        plan: &LspLaunchPlan,
        operation: impl FnOnce(&mut LspStdioClient) -> AxonResult<T>,
    ) -> AxonResult<T> {
        with_client_internal(plan, None, operation)
    }

    /// 在调用方给定的总 deadline 内复用或启动指定 LSP client。
    pub(crate) fn with_client_until<T>(
        plan: &LspLaunchPlan,
        deadline: Instant,
        operation: impl FnOnce(&mut LspStdioClient) -> AxonResult<T>,
    ) -> AxonResult<T> {
        with_client_internal(plan, Some(deadline), operation)
    }

    /// 返回 pool 统计快照。
    ///
    #[must_use]
    pub fn stats_snapshot() -> LspClientPoolStats {
        let mut state = pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.evict_idle(Instant::now());
        state.stats.active_clients = active_client_count(&state.clients);
        state.stats
    }

    /// 关闭当前进程内所有 LSP client。
    ///
    pub fn shutdown_all() -> LspClientPoolStats {
        let clients = {
            let mut state = pool()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut state.clients)
        };
        let mut shutdowns = 0_u64;
        for slot in clients.into_values() {
            let mut entry = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if entry.shutdown() {
                shutdowns = shutdowns.saturating_add(1);
            }
        }
        let mut state = pool()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.stats.shutdowns = state.stats.shutdowns.saturating_add(shutdowns);
        state.stats.active_clients = 0;
        state.stats
    }
}

fn with_client_internal<T>(
    plan: &LspLaunchPlan,
    deadline: Option<Instant>,
    operation: impl FnOnce(&mut LspStdioClient) -> AxonResult<T>,
) -> AxonResult<T> {
    let acquired_at = Instant::now();
    let key = pool_key(plan);
    let (slot, existed) = acquire_slot(&key, acquired_at);
    let mut entry = lock_entry(&slot, deadline)?;
    let now = Instant::now();

    if entry.broken_until.is_some_and(|until| until > now) {
        record_stats(|stats| {
            stats.broken_backoffs = stats.broken_backoffs.saturating_add(1);
        });
        return Err(AxonError::tool_invalid_definition(
            "LSP server 处于 broken backoff 窗口",
        ));
    }
    entry.broken_until = None;

    if entry.client.is_none() {
        record_stats(|stats| {
            stats.starts = stats.starts.saturating_add(1);
            if existed {
                stats.spawn_coalesces = stats.spawn_coalesces.saturating_add(1);
            }
        });
        let started = deadline.map_or_else(
            || start_initialized_client(plan),
            |deadline| start_initialized_client_until(plan, deadline),
        );
        match started {
            Ok(client) => entry.client = Some(client),
            Err(error) => {
                if !deadline_elapsed(deadline) {
                    mark_broken(&mut entry);
                }
                return Err(error);
            }
        }
    } else {
        record_stats(|stats| {
            stats.reuses = stats.reuses.saturating_add(1);
        });
    }

    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        entry.last_used_at = Instant::now();
        drop(entry);
        refresh_active_client_count();
        return Err(deadline_exceeded());
    }
    let result = entry.client.as_mut().map_or_else(
        || {
            Err(AxonError::tool_invalid_definition(
                "LSP client pool 状态缺失",
            ))
        },
        operation,
    );
    entry.last_used_at = Instant::now();
    if result.is_err() && !deadline_elapsed(deadline) {
        mark_broken(&mut entry);
    }
    drop(entry);
    refresh_active_client_count();
    result
}

fn deadline_elapsed(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|deadline| Instant::now() >= deadline)
}

fn mark_broken(entry: &mut LspClientPoolEntry) {
    entry.shutdown();
    entry.broken_until = Some(Instant::now() + LSP_CLIENT_BROKEN_BACKOFF);
    record_stats(|stats| {
        stats.broken_marks = stats.broken_marks.saturating_add(1);
    });
}

fn lock_entry<'a>(
    slot: &'a Mutex<LspClientPoolEntry>,
    deadline: Option<Instant>,
) -> AxonResult<MutexGuard<'a, LspClientPoolEntry>> {
    let Some(deadline) = deadline else {
        return Ok(slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner()));
    };
    loop {
        match slot.try_lock() {
            Ok(entry) => return Ok(entry),
            Err(TryLockError::Poisoned(poisoned)) => return Ok(poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(deadline_exceeded());
                }
                thread::park_timeout(remaining.min(LSP_CLIENT_LOCK_RETRY));
            }
        }
    }
}

fn start_initialized_client(plan: &LspLaunchPlan) -> AxonResult<LspStdioClient> {
    let root = PathBuf::from(&plan.cwd);
    let mut client = LspStdioClient::start(plan)?;
    client.initialize(&LspClientInitializeInput {
        root_uri: path_to_file_uri(&root)?,
        process_id: std::process::id(),
    })?;
    Ok(client)
}

fn start_initialized_client_until(
    plan: &LspLaunchPlan,
    deadline: Instant,
) -> AxonResult<LspStdioClient> {
    let root = PathBuf::from(&plan.cwd);
    let mut client = LspStdioClient::start(plan)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(deadline_exceeded());
    }
    client.initialize_with_timeout(
        &LspClientInitializeInput {
            root_uri: path_to_file_uri(&root)?,
            process_id: std::process::id(),
        },
        remaining,
    )?;
    Ok(client)
}

fn deadline_exceeded() -> AxonError {
    AxonError::tool_invalid_definition(LSP_CLIENT_DEADLINE_EXCEEDED)
}

fn pool_key(plan: &LspLaunchPlan) -> LspClientPoolKey {
    LspClientPoolKey {
        workspace_root: stable_workspace_root(Path::new(&plan.cwd)),
        server_id: plan.server_id.clone(),
    }
}

fn stable_workspace_root(workspace_root: &Path) -> String {
    workspace_root
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(workspace_root))
        .to_string_lossy()
        .replace('\\', "/")
}

fn acquire_slot(key: &LspClientPoolKey, now: Instant) -> (Arc<Mutex<LspClientPoolEntry>>, bool) {
    let mut state = pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.evict_idle(now);
    let existed = state.clients.contains_key(key);
    let slot = state
        .clients
        .entry(key.clone())
        .or_insert_with(|| Arc::new(Mutex::new(LspClientPoolEntry::new(now))))
        .clone();
    (slot, existed)
}

fn record_stats(update: impl FnOnce(&mut LspClientPoolStats)) {
    let mut state = pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    update(&mut state.stats);
}

fn refresh_active_client_count() {
    let mut state = pool()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.stats.active_clients = active_client_count(&state.clients);
}

fn active_client_count(
    clients: &HashMap<LspClientPoolKey, Arc<Mutex<LspClientPoolEntry>>>,
) -> usize {
    clients
        .values()
        .filter(|slot| match slot.try_lock() {
            Ok(entry) => entry.client.is_some(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().client.is_some(),
            Err(TryLockError::WouldBlock) => true,
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn pool_key_uses_workspace_and_server_id() {
        let workspace = PathBuf::from("C:/repo/demo");
        let plan = LspLaunchPlan {
            server_id: "rust-analyzer".to_owned(),
            program: "rust-analyzer".to_owned(),
            args: Vec::new(),
            cwd: workspace.display().to_string(),
        };

        let key = pool_key(&plan);

        assert_eq!(key.server_id, "rust-analyzer");
        assert!(key.workspace_root.ends_with("/repo/demo"));
    }

    #[test]
    fn deadline_aware_lock_stops_waiting_for_busy_slot() {
        let slot = Mutex::new(LspClientPoolEntry::new(Instant::now()));
        let _guard = slot.lock().unwrap();
        let deadline = Instant::now() + LSP_CLIENT_LOCK_RETRY;

        let result = lock_entry(&slot, Some(deadline));

        assert!(result.is_err());
        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn exhausted_deadline_does_not_mark_client_broken() {
        let _guard = pool_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-pool-deadline");
        std::fs::create_dir_all(&workspace).unwrap();
        let plan = LspLaunchPlan {
            server_id: "deadline-lsp".to_owned(),
            program: "definitely-missing-lsp-binary".to_owned(),
            args: Vec::new(),
            cwd: workspace.display().to_string(),
        };
        let _ = LspClientPoolRuntime::shutdown_all();
        let before = LspClientPoolRuntime::stats_snapshot().broken_marks;

        let result = LspClientPoolRuntime::with_client_until(&plan, Instant::now(), |_| Ok(()));
        let after = LspClientPoolRuntime::stats_snapshot().broken_marks;

        assert!(result.is_err());
        assert_eq!(after, before);
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[test]
    fn failed_start_enters_broken_backoff_window() {
        let _guard = pool_test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let workspace = axon_test_support::test_temp_root().join("axon-lsp-pool-broken");
        std::fs::create_dir_all(&workspace).unwrap();
        let plan = LspLaunchPlan {
            server_id: "missing-lsp".to_owned(),
            program: "definitely-missing-lsp-binary".to_owned(),
            args: Vec::new(),
            cwd: workspace.display().to_string(),
        };
        let _ = LspClientPoolRuntime::shutdown_all();

        let first = LspClientPoolRuntime::with_client(&plan, |_| Ok(()));
        let second = LspClientPoolRuntime::with_client(&plan, |_| Ok(()));
        let stats = LspClientPoolRuntime::stats_snapshot();

        assert!(first.is_err());
        assert!(second.is_err());
        assert!(stats.broken_marks >= 1);
        assert!(stats.broken_backoffs >= 1);
        let _ = std::fs::remove_dir_all(workspace);
    }
}
