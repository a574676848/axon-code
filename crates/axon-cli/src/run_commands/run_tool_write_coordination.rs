//! 工具访问作用域与工作区并发协调（PAR-1）。
//!
//! 对齐 DSH 的保守声明：只读调用共享工作区读闸并行；写、编辑以及无法判定
//! 读写的调用一律独占，并走工作区写闸。不做写并行，也不做按路径加锁。

use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, RwLock, Weak},
};

use axon_agent_runtime::ShellCommandPolicyEvaluator;
use axon_domain::ToolConcurrencyPolicy;

use crate::tool_orchestrator::ToolExecutionRequest;

/// 执行域：只保留三档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExecutionDomain {
    /// 不触碰磁盘，免锁并行。
    ParallelSafe,
    /// 只读磁盘，共享工作区读闸。
    ReadPaths,
    /// 写或无法判定，独占且前后加屏障。
    Exclusive,
}

/// 一次工具调用的访问作用域。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolAccessScope {
    pub(super) domain: ExecutionDomain,
    /// 独占调用是否还要与工作区写者互斥（写、编辑、未知写入集为真）。
    exclusive_workspace: bool,
}

impl ToolAccessScope {
    fn parallel_safe() -> Self {
        Self {
            domain: ExecutionDomain::ParallelSafe,
            exclusive_workspace: false,
        }
    }

    fn read_paths() -> Self {
        Self {
            domain: ExecutionDomain::ReadPaths,
            exclusive_workspace: false,
        }
    }

    fn exclusive() -> Self {
        Self {
            domain: ExecutionDomain::Exclusive,
            exclusive_workspace: false,
        }
    }

    /// 写与未知盘面访问：独占且取工作区写闸。
    fn exclusive_over_workspace() -> Self {
        Self {
            domain: ExecutionDomain::Exclusive,
            exclusive_workspace: true,
        }
    }

    /// 是否需要前后屏障（独占调用不能被邻居夹在中间）。
    pub(super) fn is_isolated(&self) -> bool {
        self.domain == ExecutionDomain::Exclusive
    }
}

#[derive(Debug, Default)]
pub(super) struct WorkspaceWriteCoordinator {
    workspace: RwLock<()>,
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
/// 判定只看本次调用的参数：声明为 `Exclusive` 不降级；`WorkspaceCoordinated`
/// 只有明确只读才算并行；读盘但推导不出读路径按独占处理。
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
        ToolConcurrencyPolicy::WorkspaceCoordinated => {
            match name {
                "builtin.shell_command" => {
                    let evaluation =
                        ShellCommandPolicyEvaluator::evaluate(&request.tool_call.input, cwd);
                    if evaluation.is_read_only && !evaluation.affected_paths.is_empty() {
                        ToolAccessScope::read_paths()
                    } else {
                        // 写命令、重定向或读路径未知：一律独占。
                        ToolAccessScope::exclusive_over_workspace()
                    }
                }
                _ => ToolAccessScope::exclusive_over_workspace(),
            }
        }
        ToolConcurrencyPolicy::ParallelSafe => match read_intent(name, &request.tool_call.input) {
            ReadIntent::Safe => ToolAccessScope::parallel_safe(),
            ReadIntent::Paths => ToolAccessScope::read_paths(),
            ReadIntent::Unknown => ToolAccessScope::exclusive_over_workspace(),
        },
        ToolConcurrencyPolicy::Exclusive => ToolAccessScope::exclusive(),
    }
}

/// 只读工具的读意图。
enum ReadIntent {
    /// 不读盘。
    Safe,
    /// 读盘且能声明出路径。
    Paths,
    /// 读盘但推导不出路径。
    Unknown,
}

fn read_intent(name: &str, input: &axon_domain::ToolCallInput) -> ReadIntent {
    match name {
        "builtin.read_file" => {
            let Some(path) = input_json(input).and_then(|value| {
                value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(ToOwned::to_owned)
            }) else {
                return ReadIntent::Unknown;
            };
            if valid_scope(&path) {
                ReadIntent::Paths
            } else {
                ReadIntent::Unknown
            }
        }
        // glob / search 允许通配：取组件边界的静态前缀，前缀为空即读整个工作区。
        "builtin.glob" | "builtin.search" => {
            let declared = input_json(input)
                .map(|value| declared_paths_value(&value))
                .unwrap_or_default();
            if declared.is_empty() {
                return ReadIntent::Unknown;
            }
            let mut valid = true;
            for pattern in &declared {
                match static_path_prefix(pattern) {
                    Some(root) if valid_scope(&root) => {}
                    _ => {
                        valid = false;
                        break;
                    }
                }
            }
            if valid {
                ReadIntent::Paths
            } else {
                ReadIntent::Unknown
            }
        }
        _ => ReadIntent::Safe,
    }
}

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

/// 取通配模式里按组件边界成立的静态前缀；第一个组件就含通配符时返回 `None`。
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

/// 只接受仓库相对路径：拒绝绝对路径、`..` 与通配符。
fn valid_scope(scope: &str) -> bool {
    let trimmed = scope.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.contains("..")
        || trimmed.contains(['*', '?', '[', '{'])
    {
        return false;
    }
    let mut chars = trimmed.chars();
    !matches!(
        (chars.next(), chars.next()),
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

fn input_json(input: &axon_domain::ToolCallInput) -> Option<serde_json::Value> {
    if let Some(value) = input.as_json() {
        return Some(value.clone());
    }
    serde_json::from_str(input.as_text()?).ok()
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
