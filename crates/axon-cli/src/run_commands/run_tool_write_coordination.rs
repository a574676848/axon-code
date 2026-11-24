//! 工具访问作用域与工作区并发协调（PAR-1）。
//!
//! 写调用按**路径**协调：`builtin.edit` 与写入集可静态判定的 shell 命令只占用自己
//! 触碰的路径，所以不同文件的写可以在同一批次里并行；同一文件的读与写、写与写仍
//! 严格互斥。写入集无法判定的调用（构建、包管理、脚本解释器）回退整工作区写闸，
//! 与所有读者和写者互斥，这是并发正确性的兜底下限，不是可调策略。
//!
//! 锁的覆盖边界：路径锁只保护**在本次调用内完成**的写。`run_in_background` 的作业
//! 与 `tty` 交互会话在 `execute` 返回后继续运行，它们的写靠 `write_stdin` 的整工作区
//! 写闸与调用方自律兜底；路径锁不为这两条通道背书。

use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{Arc, Condvar, Mutex, OnceLock, RwLock, Weak},
};

use axon_agent_runtime::ShellCommandPolicyEvaluator;
use axon_domain::{ToolCallInput, ToolConcurrencyPolicy};

use crate::tool_orchestrator::ToolExecutionRequest;

/// 执行域：四档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExecutionDomain {
    /// 不触碰磁盘，免锁并行。
    ParallelSafe,
    /// 只读磁盘，共享工作区读闸；能确定读路径时再加路径读位。
    ReadPaths,
    /// 写已解析出的路径集合，只与同一路径的访问互斥。
    WritePaths,
    /// 写或无法判定，独占且前后加屏障。
    Exclusive,
}

/// 一次工具调用的访问作用域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolAccessScope {
    pub(super) domain: ExecutionDomain,
    /// 独占调用是否还要与工作区写者互斥（写、编辑、未知写入集为真）。
    exclusive_workspace: bool,
    /// 已规范化路径：`WritePaths` 是写集，`ReadPaths` 是可确定的读集。
    paths: Vec<PathBuf>,
}

impl ToolAccessScope {
    fn parallel_safe() -> Self {
        Self {
            domain: ExecutionDomain::ParallelSafe,
            exclusive_workspace: false,
            paths: Vec::new(),
        }
    }

    fn over_read_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            domain: ExecutionDomain::ReadPaths,
            exclusive_workspace: false,
            paths,
        }
    }

    fn over_write_paths(paths: Vec<PathBuf>) -> Self {
        Self {
            domain: ExecutionDomain::WritePaths,
            exclusive_workspace: false,
            paths,
        }
    }

    fn exclusive() -> Self {
        Self {
            domain: ExecutionDomain::Exclusive,
            exclusive_workspace: false,
            paths: Vec::new(),
        }
    }

    /// 写与未知盘面访问：独占且取工作区写闸。
    fn exclusive_over_workspace() -> Self {
        Self {
            domain: ExecutionDomain::Exclusive,
            exclusive_workspace: true,
            paths: Vec::new(),
        }
    }

    /// 是否需要前后屏障（独占调用不能被邻居夹在中间）。
    pub(super) fn is_isolated(&self) -> bool {
        self.domain == ExecutionDomain::Exclusive
    }

    /// 该调用的写集；非写域为空。
    pub(super) fn write_paths(&self) -> &[PathBuf] {
        if self.domain == ExecutionDomain::WritePaths {
            &self.paths
        } else {
            &[]
        }
    }

    /// 该调用的读集；非读域为空。
    pub(super) fn read_paths(&self) -> &[PathBuf] {
        if self.domain == ExecutionDomain::ReadPaths {
            &self.paths
        } else {
            &[]
        }
    }

    /// 与同段已累积的读写集合是否冲突：写撞写、写撞读、读撞写都必须在段边界断开。
    pub(super) fn conflicts_with(
        &self,
        writes: &HashSet<PathBuf>,
        reads: &HashSet<PathBuf>,
    ) -> bool {
        self.write_paths()
            .iter()
            .any(|path| writes.contains(path) || reads.contains(path))
            || self.read_paths().iter().any(|path| writes.contains(path))
    }
}

#[derive(Debug, Default)]
pub(super) struct WorkspaceWriteCoordinator {
    /// 整工作区闸：不可判定的写取写闸，其余读写共享读闸。
    workspace: RwLock<()>,
    /// 路径占用表：同路径互斥，不同路径互不影响。
    paths: PathOccupancy,
}

impl WorkspaceWriteCoordinator {
    pub(super) fn execute<T>(&self, scope: &ToolAccessScope, operation: impl FnOnce() -> T) -> T {
        match scope.domain {
            ExecutionDomain::ParallelSafe => operation(),
            ExecutionDomain::ReadPaths => {
                let _workspace = self
                    .workspace
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _paths = self.paths.enter(&scope.paths, false);
                operation()
            }
            ExecutionDomain::WritePaths => {
                let _workspace = self
                    .workspace
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let _paths = self.paths.enter(&scope.paths, true);
                operation()
            }
            ExecutionDomain::Exclusive => {
                if scope.exclusive_workspace {
                    let _workspace = self
                        .workspace
                        .write()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    operation()
                } else {
                    operation()
                }
            }
        }
    }
}

/// 路径占用表。
///
/// 用「一次原子占用全部路径」取代多把锁逐把获取：判断与登记在同一把互斥量内完成，
/// 因此不需要锁排序，也不会出现两个写者各持一半路径的死锁。
#[derive(Debug, Default)]
struct PathOccupancy {
    state: Mutex<PathOccupancyState>,
    released: Condvar,
}

#[derive(Debug, Default)]
struct PathOccupancyState {
    /// 读位：同一路径允许并发读。
    readers: HashMap<PathBuf, usize>,
    /// 写位：同一路径同时只允许一个写者。
    writers: HashSet<PathBuf>,
}

impl PathOccupancy {
    fn enter(&self, paths: &[PathBuf], exclusive: bool) -> PathOccupancyGuard<'_> {
        if paths.is_empty() {
            return PathOccupancyGuard {
                occupancy: self,
                paths: Vec::new(),
                exclusive,
            };
        }
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            let ready = if exclusive {
                paths.iter().all(|path| {
                    !state.writers.contains(path)
                        && state.readers.get(path).copied().unwrap_or(0) == 0
                })
            } else {
                paths.iter().all(|path| !state.writers.contains(path))
            };
            if ready {
                if exclusive {
                    for path in paths {
                        state.writers.insert(path.clone());
                    }
                } else {
                    for path in paths {
                        *state.readers.entry(path.clone()).or_insert(0) += 1;
                    }
                }
                return PathOccupancyGuard {
                    occupancy: self,
                    paths: paths.to_vec(),
                    exclusive,
                };
            }
            state = self
                .released
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

struct PathOccupancyGuard<'a> {
    occupancy: &'a PathOccupancy,
    paths: Vec<PathBuf>,
    exclusive: bool,
}

impl Drop for PathOccupancyGuard<'_> {
    fn drop(&mut self) {
        if self.paths.is_empty() {
            return;
        }
        let mut state = self
            .occupancy
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.exclusive {
            for path in &self.paths {
                state.writers.remove(path);
            }
        } else {
            for path in &self.paths {
                if let Some(count) = state.readers.get_mut(path) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        state.readers.remove(path);
                    }
                }
            }
        }
        drop(state);
        self.occupancy.released.notify_all();
    }
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

/// 判定一次工具调用的执行域（PAR-1 第 1 节）。
///
/// 写域只在**写集可确定**时成立：`builtin.edit` 的补丁与哈希头给出目标文件，
/// shell 命令只有写入集可证完整时才收窄。其余写与未知访问独占据写闸。
pub(super) fn tool_access_scope(cwd: &Path, request: &ToolExecutionRequest) -> ToolAccessScope {
    if request.concurrency_policy == ToolConcurrencyPolicy::Exclusive {
        // 写进程输入会改运行态，必须与工作区写者互斥；控制类工具不加闸。
        if request.tool_call.dispatch_name() == "builtin.write_stdin" {
            return ToolAccessScope::exclusive_over_workspace();
        }
        return ToolAccessScope::exclusive();
    }
    let name = request.tool_call.dispatch_name();
    match request.concurrency_policy {
        ToolConcurrencyPolicy::WorkspaceCoordinated => match name {
            "builtin.edit" => edit_scope(cwd, &request.tool_call.input),
            "builtin.shell_command" => shell_scope(cwd, &request.tool_call.input),
            _ => ToolAccessScope::exclusive_over_workspace(),
        },
        ToolConcurrencyPolicy::ParallelSafe => {
            if is_disk_read_tool(name) {
                ToolAccessScope::over_read_paths(read_tool_paths(
                    cwd,
                    name,
                    &request.tool_call.input,
                ))
            } else {
                ToolAccessScope::parallel_safe()
            }
        }
        ToolConcurrencyPolicy::Exclusive => ToolAccessScope::exclusive(),
    }
}

/// edit 的写集来自补丁 hunk 或哈希头；解析不出来就没有并发资格。
fn edit_scope(cwd: &Path, input: &ToolCallInput) -> ToolAccessScope {
    match axon_agent_runtime::agent_run_edit_write_paths_from_provider_input(input) {
        Ok(paths) => {
            let paths = normalized_paths(cwd, &paths);
            if paths.is_empty() {
                ToolAccessScope::exclusive_over_workspace()
            } else {
                ToolAccessScope::over_write_paths(paths)
            }
        }
        // 写集不可判定时回退整工作区；绝不把解析失败当成"没有写入"。
        Err(_) => ToolAccessScope::exclusive_over_workspace(),
    }
}

fn shell_scope(cwd: &Path, input: &ToolCallInput) -> ToolAccessScope {
    let evaluation = ShellCommandPolicyEvaluator::evaluate(input, cwd);
    if evaluation.is_read_only {
        return ToolAccessScope::over_read_paths(Vec::new());
    }
    if evaluation.write_paths_are_complete {
        // `affected_paths` 已经在求值阶段按命令自身 cwd（`workdir` 参数与 `cd X;`
        // 前缀）拼好，这里统一按会话 cwd 归一化即可，不能再叠加一次命令 cwd。
        let paths = normalized_paths(cwd, &evaluation.affected_paths);
        return if paths.is_empty() {
            // 声明写入集完整却枚举不出路径：命令实际不写盘，按只读处理。
            ToolAccessScope::over_read_paths(Vec::new())
        } else {
            ToolAccessScope::over_write_paths(paths)
        };
    }
    ToolAccessScope::exclusive_over_workspace()
}

/// 声明为 `ParallelSafe` 且确实读盘的只读工具。
fn is_disk_read_tool(name: &str) -> bool {
    matches!(
        name,
        "builtin.read_file" | "builtin.glob" | "builtin.search"
    )
}

/// 只有 `read_file` 的读面是单个具体文件；`glob` / `search` 的读面是模式，无法枚举，
/// 因此它们只取工作区读闸、不加路径读位：与不同文件的写可以并发，与同目录的写并发时
/// 可能读到编辑过程中的中间态。
fn read_tool_paths(cwd: &Path, name: &str, input: &ToolCallInput) -> Vec<PathBuf> {
    if name != "builtin.read_file" {
        return Vec::new();
    }
    read_file_path(cwd, input).into_iter().collect()
}

fn read_file_path(cwd: &Path, input: &ToolCallInput) -> Option<PathBuf> {
    // 复用执行侧解析器：JSON 与文本两种输入形态给出同一结论，避免自定义解析漏形态。
    let parsed = axon_agent_runtime::agent_run_read_file_tool_input_from_provider_args(
        cwd,
        &input.canonical_text(),
    )
    .ok()?;
    Some(normalized_path(cwd, &parsed.path))
}

fn normalized_paths<I, P>(cwd: &Path, paths: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut normalized = paths
        .into_iter()
        .map(|path| normalized_path(cwd, path.as_ref()))
        .collect::<Vec<_>>();
    normalized.sort();
    normalized.dedup();
    normalized
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
