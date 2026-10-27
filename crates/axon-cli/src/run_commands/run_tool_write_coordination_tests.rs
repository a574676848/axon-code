//! `run_tool_write_coordination` 的单元测试。
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
fn classifier_maps_declared_policy_to_execution_domain() {
    let cwd = Path::new(".");
    let cases = [
        (
            "builtin.memory_search",
            serde_json::json!({}),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ParallelSafe,
        ),
        (
            "builtin.read_file",
            serde_json::json!({ "path": "src/lib.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
        ),
        (
            // 只读命令但 evaluator 推导不出读路径：按 `S-7.1` 判独占，不猜路径。
            "builtin.shell_command",
            serde_json::json!({ "command": "grep hello src/lib.rs" }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.edit",
            serde_json::json!({
                "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-a\n+b\n*** End Patch"
            }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::WritePaths,
        ),
        (
            "builtin.shell_command",
            serde_json::json!({ "command": "cargo test > out.log" }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::ExclusiveWrite,
        ),
        (
            "builtin.update_plan",
            serde_json::json!({}),
            ToolConcurrencyPolicy::Exclusive,
            ExecutionDomain::Exclusive,
        ),
        (
            "unknown.tool",
            serde_json::json!({}),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::Exclusive,
        ),
        (
            // 读目录但调用方未声明 paths：按 `S-7.1` fail-closed 到独占。
            "builtin.glob",
            serde_json::json!({ "pattern": "**/*.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.glob",
            serde_json::json!({ "pattern": "**/*.rs", "paths": ["src"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
        ),
        (
            // 通配模式取静态前缀 `src` 作为读根，前缀解析不出时 fail-closed。
            "builtin.glob",
            serde_json::json!({ "paths": ["src/**/*.rs"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
        ),
        (
            "builtin.search",
            serde_json::json!({ "paths": "**/*.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::Exclusive,
        ),
        (
            // 前缀按组件边界截断：`crates/axon-*/src/**` 的读根是 `crates`。
            "builtin.glob",
            serde_json::json!({ "paths": ["crates/axon-*/src/**"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
        ),
        (
            // 首个组件就含通配符：读取根无法收窄，按整工作区 fail-closed。
            "builtin.glob",
            serde_json::json!({ "paths": ["Cargo*"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::Exclusive,
        ),
    ];
    for (name, arguments, policy, expected) in cases {
        let scope = tool_access_scope(
            cwd,
            &request(name, &arguments, policy, ToolExecutionPermission::Allowed),
        );
        assert_eq!(scope.domain, expected, "{name} 执行域判定错误");
    }
}

#[test]
fn read_and_write_on_same_path_are_mutually_exclusive() {
    let cwd = Path::new(".");
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    let read = tool_access_scope(
        cwd,
        &request(
            "builtin.read_file",
            &serde_json::json!({ "path": "src/lib.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ToolExecutionPermission::Allowed,
        ),
    );
    let write = tool_access_scope(
        cwd,
        &request(
            "builtin.edit",
            &serde_json::json!({
                "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-a\n+b\n*** End Patch"
            }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ToolExecutionPermission::Allowed,
        ),
    );
    assert!(read.conflicts_with(&write));
    assert_scope_blocking(Arc::clone(&coordinator), read, write, true);
}

/// A-02 夹具形状：同路径读写重复 20 次，执行窗口必须零重叠。
#[test]
fn read_write_windows_never_overlap_across_repeated_batches() {
    let cwd = Path::new(".");
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    let read = tool_access_scope(
        cwd,
        &request(
            "builtin.read_file",
            &serde_json::json!({ "path": "src/lib.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ToolExecutionPermission::Allowed,
        ),
    );
    let write = tool_access_scope(
        cwd,
        &request(
            "builtin.edit",
            &serde_json::json!({
                "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-a\n+b\n*** End Patch"
            }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ToolExecutionPermission::Allowed,
        ),
    );
    for _ in 0..20 {
        assert_scope_blocking(Arc::clone(&coordinator), read.clone(), write.clone(), true);
        assert_scope_blocking(Arc::clone(&coordinator), write.clone(), read.clone(), true);
    }
}

#[test]
fn parent_directory_write_excludes_nested_path_access() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    let directory_write = write_scope(&["nd"]);
    let nested_write = write_scope(&["nd/x.rs"]);
    assert!(directory_write.conflicts_with(&nested_write));
    assert_scope_blocking(
        Arc::clone(&coordinator),
        directory_write,
        nested_write,
        true,
    );
}

#[test]
fn distinct_paths_remain_parallel_while_same_paths_serialize() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(
        Arc::clone(&coordinator),
        write_scope(&["same.rs"]),
        write_scope(&["same.rs"]),
        true,
    );
    assert_scope_blocking(
        Arc::clone(&coordinator),
        write_scope(&["left.rs"]),
        write_scope(&["right.rs"]),
        false,
    );
    assert_scope_blocking(
        coordinator,
        read_scope(&["left.rs"]),
        read_scope(&["left.rs"]),
        false,
    );
}

#[test]
fn conflicts_detected_for_overlapping_scopes() {
    let reads = read_scope(&["src"]);
    let nested_write = write_scope(&["src/lib.rs"]);
    let disjoint = write_scope(&["docs/readme.md"]);
    let workspace_write = ToolAccessScope::exclusive_write();

    assert!(reads.conflicts_with(&nested_write));
    assert!(!reads.conflicts_with(&disjoint));
    assert!(workspace_write.conflicts_with(&reads));
    assert!(!workspace_write.conflicts_with(&ToolAccessScope::parallel_safe()));
    assert!(ToolAccessScope::exclusive().conflicts_with(&ToolAccessScope::parallel_safe()));
}

#[test]
fn shell_write_scope_narrows_only_when_redirect_targets_are_the_complete_write_set() {
    let cwd = Path::new(".");
    let scope = |script: &str| {
        tool_access_scope(
            cwd,
            &request(
                "builtin.shell_command",
                &serde_json::json!({ "command": script }),
                ToolConcurrencyPolicy::WorkspaceCoordinated,
                ToolExecutionPermission::Allowed,
            ),
        )
        .domain
    };

    // 纯输出程序构成的重定向：目标即完整写入集，允许收窄。
    assert_eq!(
        scope("echo x > a.log && echo y > b.log"),
        ExecutionDomain::WritePaths
    );

    // 命令自身还会写别处（cargo 写 target/、npm 写 node_modules/、
    // 未知写入），收窄等于向协调器谎报写入范围，必须回退整工作区。
    for script in [
        "cargo test > out.log",
        "cargo build 2> err.txt",
        "npm install > install.log",
        "cargo test | tee out.log",
        "echo x > $TARGET; cargo test > out.log",
    ] {
        assert_eq!(
            scope(script),
            ExecutionDomain::ExclusiveWrite,
            "{script} 的写入集不完整，不得收窄写作用域"
        );
    }
}

fn read_scope(paths: &[&str]) -> ToolAccessScope {
    ToolAccessScope::read_paths(paths.iter().map(PathBuf::from).collect())
}

fn write_scope(paths: &[&str]) -> ToolAccessScope {
    ToolAccessScope::write_paths(paths.iter().map(PathBuf::from).collect())
}

fn assert_scope_blocking(
    coordinator: Arc<WorkspaceWriteCoordinator>,
    first: ToolAccessScope,
    second: ToolAccessScope,
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
