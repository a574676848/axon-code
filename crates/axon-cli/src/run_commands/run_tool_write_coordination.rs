//! 工具写入作用域与工作区并发协调。

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolWriteScope {
    ReadOnly,
    Paths(Vec<PathBuf>),
    Workspace,
}

#[derive(Debug, Default)]
pub(super) struct WorkspaceWriteCoordinator {
    workspace: RwLock<()>,
    path_locks: Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl WorkspaceWriteCoordinator {
    pub(super) fn execute<T>(&self, scope: &ToolWriteScope, operation: impl FnOnce() -> T) -> T {
        match scope {
            ToolWriteScope::ReadOnly => operation(),
            ToolWriteScope::Workspace => {
                let _workspace = self
                    .workspace
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                operation()
            }
            ToolWriteScope::Paths(paths) => {
                let _workspace = self
                    .workspace
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let locks = self.path_locks(paths);
                let _path_guards = locks
                    .iter()
                    .map(|lock| {
                        lock.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                    })
                    .collect::<Vec<_>>();
                operation()
            }
        }
    }

    fn path_locks(&self, paths: &[PathBuf]) -> Vec<Arc<Mutex<()>>> {
        let mut registry = self
            .path_locks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        paths
            .iter()
            .map(|path| {
                Arc::clone(
                    registry
                        .entry(path.clone())
                        .or_insert_with(|| Arc::new(Mutex::new(()))),
                )
            })
            .collect()
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

pub(super) fn tool_write_scope(cwd: &Path, request: &ToolExecutionRequest) -> ToolWriteScope {
    if request.concurrency_policy == ToolConcurrencyPolicy::ParallelSafe {
        return ToolWriteScope::ReadOnly;
    }
    let paths = match request.tool_call.dispatch_name() {
        "builtin.shell_command" => {
            let evaluation = ShellCommandPolicyEvaluator::evaluate(&request.tool_call.input, cwd);
            if evaluation.is_read_only {
                return ToolWriteScope::ReadOnly;
            }
            if !evaluation.write_paths_are_complete {
                // 重定向目标不构成完整写入集（如 `cargo test > out.log` 还会写 target/），
                // 收窄等于向协调器谎报写入范围，故回退整工作区互斥。
                return ToolWriteScope::Workspace;
            }
            Ok(evaluation
                .affected_paths
                .into_iter()
                .map(|path| path.display().to_string())
                .collect())
        }
        "builtin.edit" => agent_run_edit_write_paths_from_provider_input(&request.tool_call.input),
        _ => return ToolWriteScope::Workspace,
    };
    let Ok(paths) = paths else {
        return ToolWriteScope::Workspace;
    };
    let paths = paths
        .into_iter()
        .map(|path| normalized_path(cwd, Path::new(&path)))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if paths.is_empty() {
        ToolWriteScope::Workspace
    } else {
        ToolWriteScope::Paths(paths)
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
mod tests {
    use std::{
        sync::{Arc, mpsc},
        thread,
        time::Duration,
    };

    use axon_agent_runtime::ProviderToolCall;
    use axon_domain::{ExecutionRequestId, ToolCallId, ToolExecutionPermission};

    use crate::tool_orchestrator::ToolKind;

    use super::*;

    #[test]
    fn shell_write_scope_narrows_only_when_redirect_targets_are_the_complete_write_set() {
        let cwd = Path::new("E:/workspace");
        let scope = |script: &str| {
            tool_write_scope(
                cwd,
                &request(
                    "builtin.shell_command",
                    &serde_json::json!({ "command": script }),
                    ToolConcurrencyPolicy::WorkspaceCoordinated,
                    ToolExecutionPermission::Allowed,
                ),
            )
        };

        // 纯输出程序构成的重定向：目标即完整写入集，允许收窄。
        let ToolWriteScope::Paths(paths) = scope("echo x > a.log && echo y > b.log") else {
            panic!("纯输出程序的重定向应收窄为路径作用域");
        };
        assert_eq!(paths.len(), 2);

        // 命令自身还会写别处（cargo 写 target/、npm 写 node_modules/、
        // 段 1 的未知写入），收窄等于向协调器谎报写入范围，必须回退整工作区。
        for script in [
            "cargo test > out.log",
            "cargo build 2> err.txt",
            "npm install > install.log",
            "cargo test | tee out.log",
            "echo x > $TARGET; cargo test > out.log",
        ] {
            assert_eq!(
                scope(script),
                ToolWriteScope::Workspace,
                "{script} 的写入集不完整，不得收窄写作用域"
            );
        }
    }

    #[test]
    fn write_scope_extracts_structured_paths_and_falls_back_to_workspace() {
        let cwd = Path::new("E:/workspace");
        let patch = request(
            "builtin.edit",
            &serde_json::json!({
                "input": "*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n-old\n+new\n*** End Patch"
            }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ToolExecutionPermission::Allowed,
        );
        let shell = ToolExecutionRequest {
            tool_call: ProviderToolCall::new(
                "builtin.shell_command",
                "builtin.shell_command",
                axon_domain::ToolCallInput::text("echo ok"),
            ),
            tool_call_id: ToolCallId(1),
            part_id: "tool-part:test:builtin.shell_command".to_owned(),
            execution_request_id: ExecutionRequestId(1),
            kind: ToolKind::Shell,
            concurrency_policy: ToolConcurrencyPolicy::WorkspaceCoordinated,
            execution_permission: ToolExecutionPermission::Allowed,
        };
        let read = request(
            "builtin.read_file",
            &serde_json::json!({"path": "old.rs"}),
            ToolConcurrencyPolicy::ParallelSafe,
            ToolExecutionPermission::Allowed,
        );
        let governed_write = request(
            "mcp.command",
            &serde_json::json!({}),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ToolExecutionPermission::Allowed,
        );

        let ToolWriteScope::Paths(paths) = tool_write_scope(cwd, &patch) else {
            panic!("edit patch 应解析为路径写作用域");
        };
        assert_eq!(paths.len(), 2);
        assert_eq!(tool_write_scope(cwd, &shell), ToolWriteScope::Workspace);
        assert_eq!(tool_write_scope(cwd, &read), ToolWriteScope::ReadOnly);
        assert_eq!(
            tool_write_scope(cwd, &governed_write),
            ToolWriteScope::Workspace
        );
    }

    #[test]
    fn same_path_writes_serialize_while_distinct_paths_remain_parallel() {
        let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
        assert_scope_blocking(
            Arc::clone(&coordinator),
            ToolWriteScope::Paths(vec![PathBuf::from("same.rs")]),
            ToolWriteScope::Paths(vec![PathBuf::from("same.rs")]),
            true,
        );
        assert_scope_blocking(
            coordinator,
            ToolWriteScope::Paths(vec![PathBuf::from("left.rs")]),
            ToolWriteScope::Paths(vec![PathBuf::from("right.rs")]),
            false,
        );
    }

    fn assert_scope_blocking(
        coordinator: Arc<WorkspaceWriteCoordinator>,
        first: ToolWriteScope,
        second: ToolWriteScope,
        should_block: bool,
    ) {
        let (first_entered_tx, first_entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_coordinator = Arc::clone(&coordinator);
        let first_handle = thread::spawn(move || {
            first_coordinator.execute(&first, || {
                first_entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
        });
        first_entered_rx.recv().unwrap();

        let (second_entered_tx, second_entered_rx) = mpsc::channel();
        let second_handle = thread::spawn(move || {
            coordinator.execute(&second, || second_entered_tx.send(()).unwrap());
        });
        let entered_before_release = second_entered_rx.recv_timeout(Duration::from_millis(100));
        assert_eq!(entered_before_release.is_err(), should_block);
        release_tx.send(()).unwrap();
        if should_block {
            second_entered_rx
                .recv_timeout(Duration::from_secs(1))
                .unwrap();
        }
        first_handle.join().unwrap();
        second_handle.join().unwrap();
    }

    fn request(
        name: &str,
        arguments: &serde_json::Value,
        concurrency_policy: ToolConcurrencyPolicy,
        execution_permission: ToolExecutionPermission,
    ) -> ToolExecutionRequest {
        ToolExecutionRequest {
            tool_call: ProviderToolCall::new(name, name, arguments.to_string()),
            tool_call_id: ToolCallId(1),
            part_id: format!("tool-part:test:{name}"),
            execution_request_id: ExecutionRequestId(1),
            kind: ToolKind::Unsupported,
            concurrency_policy,
            execution_permission,
        }
    }
}
