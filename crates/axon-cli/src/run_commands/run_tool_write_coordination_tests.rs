//! `run_tool_write_coordination` 的单元测试（PAR-1）。

use std::{
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use axon_agent_runtime::ProviderToolCall;
use axon_domain::{ExecutionRequestId, ToolCallId, ToolExecutionPermission};

use crate::tool_orchestrator::ToolKind;

use super::*;

fn request(
    name: &str,
    arguments: &serde_json::Value,
    policy: ToolConcurrencyPolicy,
) -> ToolExecutionRequest {
    ToolExecutionRequest {
        tool_call: ProviderToolCall::new(name, name, arguments.to_string()),
        tool_call_id: ToolCallId(1),
        part_id: format!("tool-part:test:{name}"),
        execution_request_id: ExecutionRequestId(1),
        kind: ToolKind::Unsupported,
        concurrency_policy: policy,
        execution_permission: ToolExecutionPermission::Allowed,
    }
}

fn scope(
    name: &str,
    arguments: serde_json::Value,
    policy: ToolConcurrencyPolicy,
) -> ToolAccessScope {
    tool_access_scope(Path::new("."), &request(name, &arguments, policy))
}

#[test]
fn classifier_maps_calls_to_three_domains() {
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
            "builtin.read_file",
            serde_json::json!({ "path": "../escape.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.glob",
            serde_json::json!({ "pattern": "**/*.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.glob",
            serde_json::json!({ "paths": ["src/**/*.rs"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
        ),
        (
            "builtin.glob",
            serde_json::json!({ "paths": ["{src,docs}/**"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.edit",
            serde_json::json!({ "input": "*** Begin Patch\n*** Update File: src/lib.rs\n*** End Patch" }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.shell_command",
            serde_json::json!({ "command": "cargo test > out.log" }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::Exclusive,
        ),
        (
            "unknown.tool",
            serde_json::json!({}),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::Exclusive,
        ),
        (
            "builtin.update_plan",
            serde_json::json!({}),
            ToolConcurrencyPolicy::Exclusive,
            ExecutionDomain::Exclusive,
        ),
    ];
    for (name, arguments, policy, expected) in cases {
        assert_eq!(
            scope(name, arguments, policy).domain,
            expected,
            "{name} 执行域判定错误"
        );
    }
}

fn read_scope() -> ToolAccessScope {
    scope(
        "builtin.read_file",
        serde_json::json!({ "path": "src/lib.rs" }),
        ToolConcurrencyPolicy::ParallelSafe,
    )
}

fn write_scope() -> ToolAccessScope {
    scope(
        "builtin.edit",
        serde_json::json!({ "input": "*** Begin Patch\n*** Update File: src/lib.rs\n*** End Patch" }),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    )
}

#[test]
fn readers_share_the_gate_and_do_not_block_each_other() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(Arc::clone(&coordinator), read_scope(), read_scope(), false);
}

#[test]
fn writes_block_readers_and_other_writes() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(Arc::clone(&coordinator), read_scope(), write_scope(), true);
    assert_scope_blocking(Arc::clone(&coordinator), write_scope(), read_scope(), true);
    assert_scope_blocking(Arc::clone(&coordinator), write_scope(), write_scope(), true);
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
    assert_eq!(
        entered_before_release.is_err(),
        should_block,
        "并发窗口判定与预期不符"
    );
    release_tx.send(()).unwrap();
    if should_block {
        second_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
    }
    first_handle.join().unwrap();
    second_handle.join().unwrap();
}
