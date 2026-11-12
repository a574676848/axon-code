//! 前台线索与未闭合承诺的读写（M8-2 写入侧 / M8-3 读取侧）。
//!
//! * 读取：把既有的前台线索与未闭合承诺渲染成一段动态上下文，交给模型 payload；
//! * 写入：每轮进入时维护前台线索，并把承诺句式落库。
//!
//! 读写分离是硬约束：payload 构造必须可重复（预热路径与同步路径会各构造一次并逐字段
//! 比较），因此只有读侧可以进 payload，写入由每轮只执行一次的准备阶段调用。
//!
//! SQLite 句柄按线程缓存，进程内只 open 一次，避免每轮 payload 重新建连。

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

use axon_config::AxonConfig;
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig, MemorySqliteStore, P2_MAX_COMMITMENTS,
    THREADS_ENABLED_DEFAULT, ThreadScope, close_commitments, ensure_foreground, record_commitment,
    render_p2_context,
};
use axon_memory_read::MEMORY_GLOBAL_NAMESPACE_REF;

/// 本机单用户场景下线索与承诺的 owner principal。
const LOCAL_USER_REF: &str = "user:local";
/// 承诺来源渠道标识。
const COMMITMENT_CHANNEL: &str = "cli_run";

thread_local! {
    /// 进程内复用的 SQLite 句柄；键是 db 路径，切换安装根时自动重建。
    static STORE_CACHE: RefCell<Option<(PathBuf, MemorySqliteStore)>> = const { RefCell::new(None) };
}

/// 用线程缓存的 SQLite 句柄执行一次操作。
///
/// 命中缓存时不建连、不重复建表；未命中时才 open 并初始化 schema。
fn with_store<T>(
    axon_root: &Path,
    config: &AxonConfig,
    apply: impl FnOnce(&MemorySqliteStore) -> T,
) -> Option<T> {
    let memory_config = memory_config_for(axon_root, config)?;
    let expected_path = memory_config.db_path.clone();
    STORE_CACHE.with(|cache| {
        let mut cached = cache.borrow_mut();
        let reusable = cached
            .as_ref()
            .is_some_and(|(path, _)| path == &expected_path);
        if !reusable {
            let store = MemorySqliteStore::open(&memory_config).ok()?;
            store.initialize_schema().ok()?;
            *cached = Some((expected_path, store));
        }
        let (_, store) = cached.as_ref()?;
        Some(apply(store))
    })
}

/// 按 Axon 根目录与配置推导 Memory 存储位置。
fn memory_config_for(axon_root: &Path, config: &AxonConfig) -> Option<MemoryConfig> {
    let dimension = if config.memory.embedding.enabled {
        config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    };
    MemoryConfig::from_config_dir(axon_root, config.memory.embedding.enabled, dimension).ok()
}

/// 线索与承诺统一挂在全局命名空间：本机单用户、单条前台线索，跨 Workspace 延续话题。
fn local_scope() -> ThreadScope<'static> {
    ThreadScope {
        namespace_ref: MEMORY_GLOBAL_NAMESPACE_REF,
        user_ref: LOCAL_USER_REF,
    }
}

/// 读取既有线索与承诺并渲染成动态上下文段；无内容时返回 `None`。
///
/// 只读，可重复调用且逐字节一致。
pub(super) fn prepare_memory_thread_context(
    axon_root: &Path,
    config: &AxonConfig,
    tz_offset_min: i32,
) -> Option<String> {
    if !THREADS_ENABLED_DEFAULT {
        return None;
    }
    with_store(axon_root, config, |store| {
        render_existing(store, &local_scope(), tz_offset_min)
    })
    .flatten()
}

/// 维护本轮前台线索与未闭合承诺，返回新写入的承诺 ID。
///
/// 有副作用：每轮只调用一次，且必须在 payload 构造之外。
pub(super) fn maintain_memory_thread(
    axon_root: &Path,
    config: &AxonConfig,
    prompt: &str,
    now_ms: i64,
) -> Option<String> {
    if !THREADS_ENABLED_DEFAULT {
        return None;
    }
    with_store(axon_root, config, |store| {
        let scope = local_scope();
        let thread_id = ensure_foreground(store, &scope, prompt, now_ms, true).ok()?;
        let _ = close_commitments(store, &scope, prompt, now_ms, true);
        record_commitment(
            store,
            &scope,
            thread_id.as_deref(),
            prompt,
            COMMITMENT_CHANNEL,
            now_ms,
            true,
        )
        .ok()
        .flatten()
    })
    .flatten()
}

/// 渲染线索段与承诺段。
fn render_existing(
    store: &MemorySqliteStore,
    scope: &ThreadScope<'_>,
    tz_offset_min: i32,
) -> Option<String> {
    let thread = store
        .foreground_thread(scope.namespace_ref, scope.user_ref)
        .unwrap_or_default();
    let commitments = store
        .open_commitments(scope.namespace_ref, scope.user_ref, P2_MAX_COMMITMENTS)
        .unwrap_or_default();
    let rendered = render_p2_context(thread.as_ref(), &commitments, tz_offset_min);
    let rendered = rendered.trim();
    (!rendered.is_empty()).then(|| rendered.to_owned())
}

#[cfg(test)]
#[path = "run_memory_thread_context_tests.rs"]
mod tests;
