//! 工具访问作用域、五类执行域与工作区并发协调。
//!
//! 声明仍是三态 [`ToolConcurrencyPolicy`]；执行域由本模块按合同 `T-4` / `S-1`
//! 派生，声明是上界，派生只能细化不能升级。读路径取读锁、写路径取写锁，
//! 路径锁按规范化路径字典序获取，避免交叉死锁。

use std::{
    collections::{BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
};

use axon_agent_runtime::{
    ShellCommandPolicyEvaluator, agent_run_edit_write_paths_from_provider_input,
};
use axon_domain::ToolConcurrencyPolicy;

use crate::tool_orchestrator::ToolExecutionRequest;

use super::run_tool_workspace_lock::{FairWorkspaceLock, WorkspaceWriteGuard};

/// 执行域（`T-4`）：由声明派生的五类之一。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExecutionDomain {
    /// 不触碰共享状态，也不读盘。
    ParallelSafe,
    /// 读指定路径，需与写者互斥。
    ReadPaths,
    /// 写指定路径。
    WritePaths,
    /// 写整个工作区，写入集不完整。
    ExclusiveWrite,
    /// 无法安全协调，必须单独成段。
    Exclusive,
}

/// 一次工具调用的访问作用域：执行域加规范化后的读写路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolAccessScope {
    pub(super) domain: ExecutionDomain,
    pub(super) read_paths: Vec<PathBuf>,
    pub(super) write_paths: Vec<PathBuf>,
    /// 独占调用是否还要在工作区层与写者互斥。
    ///
    /// 只读但推导不出读路径的调用必须为真，否则它会与跨会话的写入并发；
    /// `update_plan` 这类不落盘的控制工具为假，避免长时间阻塞其他调用。
    exclusive_workspace: bool,
}

impl ToolAccessScope {
    fn parallel_safe() -> Self {
        Self {
            domain: ExecutionDomain::ParallelSafe,
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            exclusive_workspace: false,
        }
    }

    fn exclusive() -> Self {
        Self {
            domain: ExecutionDomain::Exclusive,
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            exclusive_workspace: false,
        }
    }

    /// 无法推导读路径的独占调用：批内独占，且与工作区写者互斥。
    fn exclusive_over_workspace() -> Self {
        Self {
            domain: ExecutionDomain::Exclusive,
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            exclusive_workspace: true,
        }
    }

    fn exclusive_write() -> Self {
        Self {
            domain: ExecutionDomain::ExclusiveWrite,
            read_paths: Vec::new(),
            write_paths: Vec::new(),
            exclusive_workspace: true,
        }
    }

    fn read_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            domain: ExecutionDomain::ReadPaths,
            read_paths: paths,
            write_paths: Vec::new(),
            exclusive_workspace: false,
        }
    }

    fn write_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            domain: ExecutionDomain::WritePaths,
            read_paths: Vec::new(),
            write_paths: paths,
            exclusive_workspace: false,
        }
    }

    /// 是否触碰磁盘（读或写）。
    pub(super) fn touches_disk(&self) -> bool {
        self.domain == ExecutionDomain::ExclusiveWrite
            || !self.read_paths.is_empty()
            || !self.write_paths.is_empty()
    }

    /// 本次访问是否需要前后屏障：独占与整工作区写都不能被邻居夹在中间。
    pub(super) fn is_isolated(&self) -> bool {
        matches!(
            self.domain,
            ExecutionDomain::Exclusive | ExecutionDomain::ExclusiveWrite
        )
    }

    /// 两个作用域是否会在同一路径上产生读写或写写交叉（合同 `S-2`）。
    pub(super) fn conflicts_with(&self, other: &Self) -> bool {
        use ExecutionDomain::{Exclusive, ExclusiveWrite, ReadPaths, WritePaths};
        match (self.domain, other.domain) {
            (Exclusive, _) | (_, Exclusive) => true,
            (ExclusiveWrite, _) => other.touches_disk(),
            (_, ExclusiveWrite) => self.touches_disk(),
            (WritePaths, WritePaths) => paths_overlap(&self.write_paths, &other.write_paths),
            (WritePaths, ReadPaths) => paths_overlap(&self.write_paths, &other.read_paths),
            (ReadPaths, WritePaths) => paths_overlap(&self.read_paths, &other.write_paths),
            _ => false,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct WorkspaceWriteCoordinator {
    workspace: FairWorkspaceLock,
    path_locks: Mutex<HashMap<PathBuf, Arc<RwLock<()>>>>,
}

impl WorkspaceWriteCoordinator {
    pub(super) fn execute<T>(&self, scope: &ToolAccessScope, operation: impl FnOnce() -> T) -> T {
        match scope.domain {
            ExecutionDomain::ParallelSafe => operation(),
            ExecutionDomain::Exclusive => {
                if scope.exclusive_workspace {
                    let _workspace = self.workspace_write();
                    operation()
                } else {
                    operation()
                }
            }
            ExecutionDomain::ExclusiveWrite => {
                let _workspace = self.workspace_write();
                operation()
            }
            ExecutionDomain::ReadPaths => {
                self.with_workspace_and_paths(&scope.read_paths, false, operation)
            }
            ExecutionDomain::WritePaths => {
                self.with_workspace_and_paths(&scope.write_paths, true, operation)
            }
        }
    }

    fn workspace_write(&self) -> WorkspaceWriteGuard<'_> {
        self.workspace.write()
    }

    /// 先取工作区读闸，再按字典序取路径锁。
    ///
    /// 祖先目录取读锁、叶子按访问方向取读/写锁：这样 `nd` 的整目录写会阻塞
    /// `nd/x.rs` 的读写，而 `src/a.rs` 与 `src/b.rs` 仍可并行。
    /// 锁的 `Arc` 与守卫都活在本次调用栈内，因此不需要自引用结构。
    fn with_workspace_and_paths<T>(
        &self,
        paths: &[PathBuf],
        exclusive: bool,
        operation: impl FnOnce() -> T,
    ) -> T {
        let _workspace = self.workspace.read();
        let entries = lock_entries(paths, exclusive);
        let locks = self.path_locks(&entries);
        let modes = entries.iter().map(|(_, write)| *write).collect::<Vec<_>>();
        let mut operation = Some(operation);
        with_path_guards(&locks, &modes, &mut operation)
    }

    fn path_locks(&self, entries: &[(PathBuf, bool)]) -> Vec<Arc<RwLock<()>>> {
        let mut registry = self
            .path_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        entries
            .iter()
            .map(|(path, _)| {
                Arc::clone(
                    registry
                        .entry(path.clone())
                        .or_insert_with(|| Arc::new(RwLock::new(()))),
                )
            })
            .collect()
    }
}

/// 归一化锁清单：祖先目录取读锁，叶子按访问方向取锁，按路径字典序排列。
///
/// 目录型读路径必须取写锁：`glob {paths:["src"]}` 读的是整棵子目录，
/// 只取 `src` 读锁会让 `src/lib.rs` 的写并发通过。
fn lock_entries(paths: &[PathBuf], exclusive: bool) -> Vec<(PathBuf, bool)> {
    let mut entries: std::collections::BTreeMap<PathBuf, bool> = std::collections::BTreeMap::new();
    for path in paths {
        for ancestor in path.ancestors().skip(1) {
            entries.entry(ancestor.to_path_buf()).or_insert(false);
        }
        let write = exclusive || path_is_directory(path);
        let leaf = entries.entry(path.clone()).or_insert(false);
        *leaf = *leaf || write;
    }
    entries.into_iter().collect()
}

/// 目录型读路径取写锁；路径不存在时按文件处理（只取读锁）。
fn path_is_directory(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

/// 按顺序逐个获取守卫，全部就绪后执行一次操作。
fn with_path_guards<T>(
    locks: &[Arc<RwLock<()>>],
    modes: &[bool],
    operation: &mut Option<impl FnOnce() -> T>,
) -> T {
    fn acquire<T>(
        locks: &[Arc<RwLock<()>>],
        modes: &[bool],
        index: usize,
        operation: &mut Option<impl FnOnce() -> T>,
    ) -> T {
        if index == locks.len() {
            let operation = operation.take().expect("操作只能执行一次");
            return operation();
        }
        if modes[index] {
            let _guard = locks[index]
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            acquire(locks, modes, index + 1, operation)
        } else {
            let _guard = locks[index]
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            acquire(locks, modes, index + 1, operation)
        }
    }
    acquire(locks, modes, 0, operation)
}

pub(super) fn workspace_write_coordinator(cwd: &Path) -> Arc<WorkspaceWriteCoordinator> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<WorkspaceWriteCoordinator>>>> =
        OnceLock::new();
    let key = normalized_path(cwd, Path::new(""));
    let mut registry = REGISTRY
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(coordinator) = registry.get(&key).and_then(Weak::upgrade) {
        return coordinator;
    }
    registry.retain(|_, coordinator| coordinator.strong_count() > 0);
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    registry.insert(key, Arc::downgrade(&coordinator));
    coordinator
}

/// 判定一次工具调用的执行域与读写路径（合同 `T-4`、`S-1`、`S-7.1`）。
///
/// 判定顺序固定：声明 `Exclusive` 不降级；`WorkspaceCoordinated` 按写入集完整性
/// 分写入域、整工作区域或只读域；`ParallelSafe` 只在声明了读路径时取读锁。
/// 只读工具的读意图：不读盘、已声明路径、或读盘但推导不出路径。
enum ReadIntent {
    Safe,
    Paths(Vec<PathBuf>),
    Unknown,
}

pub(super) fn tool_access_scope(cwd: &Path, request: &ToolExecutionRequest) -> ToolAccessScope {
    if request.concurrency_policy == ToolConcurrencyPolicy::Exclusive {
        // 写进程输入等会改运行态的工具，必须同时与工作区写者互斥；
        // update_plan / ask_user_question 这类不落盘的控制工具不加工作区闸。
        if request.tool_call.dispatch_name() == "builtin.write_stdin" {
            return ToolAccessScope::exclusive_over_workspace();
        }
        return ToolAccessScope::exclusive();
    }
    let name = request.tool_call.dispatch_name();
    match request.concurrency_policy {
        ToolConcurrencyPolicy::WorkspaceCoordinated => match name {
            "builtin.shell_command" => {
                let evaluation =
                    ShellCommandPolicyEvaluator::evaluate(&request.tool_call.input, cwd);
                if evaluation.is_read_only {
                    let paths = normalize_paths(cwd, evaluation.affected_paths);
                    // 读盘但声明不出路径：无法与写者互斥，按 `S-7.1` 判独占并加工作区屏障。
                    return if paths.is_empty() {
                        ToolAccessScope::exclusive_over_workspace()
                    } else {
                        ToolAccessScope::read_paths(paths)
                    };
                }
                if !evaluation.write_paths_are_complete {
                    // 重定向目标不构成完整写入集（如 `cargo test > out.log` 还会写 target/），
                    // 收窄等于向协调器谎报写入范围，故回退整工作区互斥。
                    return ToolAccessScope::exclusive_write();
                }
                let paths = normalize_paths(cwd, evaluation.affected_paths);
                if paths.is_empty() {
                    ToolAccessScope::exclusive_write()
                } else {
                    ToolAccessScope::write_paths(paths)
                }
            }
            "builtin.edit" => {
                let Ok(paths) =
                    agent_run_edit_write_paths_from_provider_input(&request.tool_call.input)
                else {
                    return ToolAccessScope::exclusive_write();
                };
                let paths = normalize_declared_paths(cwd, paths);
                if paths.is_empty() {
                    ToolAccessScope::exclusive_write()
                } else {
                    ToolAccessScope::write_paths(paths)
                }
            }
            // 未知工具按 `T-4` 第 4 步 fail-closed：独占，并加工作区屏障防跨会话写穿透。
            _ => ToolAccessScope::exclusive_over_workspace(),
        },
        ToolConcurrencyPolicy::ParallelSafe => {
            match read_intent(cwd, name, &request.tool_call.input) {
                ReadIntent::Safe => ToolAccessScope::parallel_safe(),
                ReadIntent::Paths(paths) => ToolAccessScope::read_paths(paths),
                ReadIntent::Unknown => ToolAccessScope::exclusive_over_workspace(),
            }
        }
        ToolConcurrencyPolicy::Exclusive => ToolAccessScope::exclusive(),
    }
}

/// 只读工具的读路径声明。
///
/// 只有已知读盘的工具（`read_file` / `glob` / `search`）需要路径声明；其余
/// 工具的声明就是 `ParallelSafe`，按 `T-4` 第 3 步直接免锁，不在此处推翻声明。
fn read_intent(cwd: &Path, name: &str, input: &axon_domain::ToolCallInput) -> ReadIntent {
    match name {
        "builtin.read_file" => input_json(input)
            .and_then(|value| {
                value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .map(|path| ReadIntent::Paths(normalize_declared_paths(cwd, vec![path])))
            .unwrap_or(ReadIntent::Unknown),
        // glob / search 默认遍历工作区，且允许通配模式：取静态前缀作为读根，
        // 前缀为空（模式以通配符开头）说明等于读整个工作区，按 fail-closed 处理。
        "builtin.glob" | "builtin.search" => {
            let declared = input_json(input)
                .map(|value| declared_paths_value(&value))
                .unwrap_or_default();
            if declared.is_empty() {
                return ReadIntent::Unknown;
            }
            let mut roots = Vec::with_capacity(declared.len());
            for pattern in declared {
                let Some(prefix) = static_path_prefix(&pattern) else {
                    return ReadIntent::Unknown;
                };
                roots.push(prefix);
            }
            ReadIntent::Paths(normalize_declared_paths(cwd, roots))
        }
        _ => ReadIntent::Safe,
    }
}

/// 取通配模式里按组件边界成立的静态前缀；第一个组件就含通配符时返回 `None`。
///
/// 与 glob 工具把简单模式归一为 `**/pattern` 的行为对齐：前缀不足以覆盖
/// 真实扫描根时，按"读整个工作区"处理。
fn static_path_prefix(pattern: &str) -> Option<String> {
    let trimmed = pattern.trim_start_matches("./");
    let mut anchor = Vec::new();
    for component in trimmed.split(['/', '\\']) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component.contains(['*', '?', '[', '{']) {
            break;
        }
        anchor.push(component);
    }
    if anchor.is_empty() {
        None
    } else {
        Some(anchor.join("/"))
    }
}

/// `paths` 既允许数组也允许单个字符串，与工具 schema 保持一致。
fn declared_paths_value(value: &serde_json::Value) -> Vec<String> {
    match value.get("paths") {
        Some(serde_json::Value::String(path)) => vec![path.clone()],
        Some(serde_json::Value::Array(paths)) => paths
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// 工具输入可能是已解析 JSON，也可能是 JSON 文本，两种都要接受。
fn input_json(input: &axon_domain::ToolCallInput) -> Option<serde_json::Value> {
    if let Some(value) = input.as_json() {
        return Some(value.clone());
    }
    serde_json::from_str(input.as_text()?).ok()
}

fn normalize_declared_paths(cwd: &Path, paths: Vec<String>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| normalized_path(cwd, Path::new(&path)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_paths(cwd: &Path, paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .map(|path| {
            if path.is_absolute() {
                normalized_path(Path::new(""), &path)
            } else {
                normalized_path(cwd, &path)
            }
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// 两组路径是否存在同一路径或父子目录交叉（按组件边界判定）。
fn paths_overlap(left: &[PathBuf], right: &[PathBuf]) -> bool {
    left.iter()
        .any(|one| right.iter().any(|other| path_is_prefix(one, other)))
}

fn path_is_prefix(ancestor: &Path, path: &Path) -> bool {
    let mut ancestor_components = ancestor.components();
    let mut path_components = path.components();
    loop {
        match (ancestor_components.next(), path_components.next()) {
            (None, _) => return true,
            (Some(expected), Some(actual)) if expected == actual => {}
            _ => return false,
        }
    }
}

fn normalized_path(cwd: &Path, relative: &Path) -> PathBuf {
    let root = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in root.join(relative).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    platform_path_key(&normalized)
}

#[cfg(windows)]
fn platform_path_key(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().to_lowercase())
}

#[cfg(not(windows))]
fn platform_path_key(path: &Path) -> PathBuf {
    path.to_path_buf()
}

#[cfg(test)]
#[path = "run_tool_write_coordination_tests.rs"]
mod tests;
