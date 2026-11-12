//! 前台线索与未闭合承诺的读取与维护（M8-2 写入侧 / M8-3 读取侧接线）。
//!
//! * 读取：把进入本轮之前的线索与未闭合承诺渲染成一段动态上下文，交给模型 payload；
//! * 写入：本轮用户输入进入后维护命名空间内的单条前台线索，并把承诺句式落库。
//!
//! 先读渲染、后写入，使本轮上下文只陈述既成事实，写入结果下一轮才可见。
//! `memory.threads.enabled = false` 时零 SQLite 打开与读写，行为与未接线前完全一致。

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use axon_config::AxonConfig;
use axon_memory::{
    MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION, MemoryConfig, MemorySqliteStore, P2_MAX_COMMITMENTS,
    THREADS_ENABLED_DEFAULT, ThreadScope, render_p2_context,
};

/// 本机单用户场景下线索与承诺的 owner principal。
const LOCAL_USER_REF: &str = "user:local";

/// 已初始化 schema 的 SQLite 文件集合；同一进程内只建表一次。
fn schema_ready_paths() -> &'static Mutex<BTreeSet<PathBuf>> {
    static READY: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    READY.get_or_init(|| Mutex::new(BTreeSet::new()))
}

/// 线索与承诺装配输入。
pub(super) struct RunMemoryThreadInput<'a> {
    /// Axon 根目录；`data/memory` 位于其下。
    pub(super) axon_root: &'a Path,
    /// 已加载配置。
    pub(super) config: &'a AxonConfig,
    /// 本轮 Memory 归属命名空间。
    pub(super) namespace_ref: &'a str,
    /// 本地时区偏移（分钟）；用于承诺时间渲染。
    pub(super) tz_offset_min: i32,
}

/// 线索与承诺装配结果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct RunMemoryThreadOutput {
    /// 动态上下文段；`None` 表示本轮无内容可注入。
    pub(super) context: Option<String>,
    /// 本轮写入的承诺 ID。
    pub(super) recorded_commitment: Option<String>,
}

/// 读取既有线索与承诺并渲染成动态上下文段。
///
/// 只读：payload 构造必须保持纯函数语义，重复调用产出必须一致。
/// 写入侧尚未接入——它需要每轮只执行一次的钩子，不能放进可重复调用的 payload 构造。
pub(super) fn prepare_memory_thread_context(
    input: &RunMemoryThreadInput<'_>,
) -> RunMemoryThreadOutput {
    if !THREADS_ENABLED_DEFAULT {
        return RunMemoryThreadOutput::default();
    }
    let Some(store) = open_store(input.axon_root, input.config) else {
        return RunMemoryThreadOutput::default();
    };
    let scope = ThreadScope {
        namespace_ref: input.namespace_ref,
        user_ref: LOCAL_USER_REF,
    };
    RunMemoryThreadOutput {
        context: render_existing(&store, &scope, input.tz_offset_min),
        recorded_commitment: None,
    }
}

/// 渲染进入本轮之前的线索段与承诺段。
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

/// 打开 SQLite 事实源，并保证 schema 在本进程内只初始化一次。
fn open_store(axon_root: &Path, config: &AxonConfig) -> Option<MemorySqliteStore> {
    let dimension = if config.memory.embedding.enabled {
        config.memory.embedding.dimension
    } else {
        MEMORY_CONFIG_DEFAULT_EMBEDDING_DIMENSION
    };
    let memory_config =
        MemoryConfig::from_config_dir(axon_root, config.memory.embedding.enabled, dimension)
            .ok()?;
    let store = MemorySqliteStore::open(&memory_config).ok()?;
    let mut ready = schema_ready_paths()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if !ready.contains(&memory_config.db_path) {
        store.initialize_schema().ok()?;
        ready.insert(memory_config.db_path.clone());
    }
    Some(store)
}

#[cfg(test)]
#[path = "run_memory_thread_context_tests.rs"]
mod tests;
