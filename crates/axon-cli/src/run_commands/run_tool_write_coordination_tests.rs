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
            ExecutionDomain::ReadPaths,
        ),
        (
            "builtin.glob",
            serde_json::json!({ "pattern": "**/*.rs" }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
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
            ExecutionDomain::ReadPaths,
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
    /// 「不该阻塞」用例的进入等待上限：并行门禁下线程调度可达数百毫秒，
    /// 窗口过窄会把「没有互斥」误判成「被阻塞」。
    const READER_ENTRY_WAIT: Duration = Duration::from_secs(10);
    /// 「该阻塞」用例的观察窗口：只需确认短时间内没有进入，
    /// 误判方向是「判成阻塞」，因此可以短。
    const BLOCK_PROBE: Duration = Duration::from_millis(100);
    let probe = if should_block {
        BLOCK_PROBE
    } else {
        READER_ENTRY_WAIT
    };
    let entered_before_release = second_entered_rx.recv_timeout(probe);
    if should_block {
        assert!(
            entered_before_release.is_err(),
            "写入必须阻塞并发访问：第二个调用在观察窗口内提前进入了"
        );
    } else {
        assert!(
            entered_before_release.is_ok(),
            "并发读不得互相阻塞：第二个调用在等待窗口内没有进入"
        );
    }
    release_tx.send(()).unwrap();
    if should_block {
        second_entered_rx.recv_timeout(READER_ENTRY_WAIT).unwrap();
    }
    first_handle.join().unwrap();
    second_handle.join().unwrap();
}

/// G2：编辑、shell 写与读取之间必须互斥（控制类独占不占工作区闸）。
#[test]
fn edits_and_shell_writes_block_reads() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    let shell_write = scope(
        "builtin.shell_command",
        serde_json::json!({ "command": "cargo test > out.log" }),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    );
    assert_scope_blocking(Arc::clone(&coordinator), write_scope(), write_scope(), true);
    assert_scope_blocking(
        Arc::clone(&coordinator),
        shell_write.clone(),
        read_scope(),
        true,
    );
    assert_scope_blocking(Arc::clone(&coordinator), read_scope(), shell_write, true);
}
