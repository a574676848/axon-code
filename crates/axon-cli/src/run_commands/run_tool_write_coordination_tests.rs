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

fn edit_patch(path: &str) -> serde_json::Value {
    serde_json::json!({
        "input": format!("*** Begin Patch\n*** Update File: {path}\n@@\n-old\n+new\n*** End Patch"),
    })
}

#[test]
fn classifier_maps_calls_to_four_domains() {
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
            serde_json::json!({ "paths": ["{src,docs}/**"] }),
            ToolConcurrencyPolicy::ParallelSafe,
            ExecutionDomain::ReadPaths,
        ),
        // 补丁写集可解析：落到路径写域，不再是整工作区独占。
        (
            "builtin.edit",
            edit_patch("src/lib.rs"),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::WritePaths,
        ),
        // 写入集可证完整的 shell 命令同样按路径写。
        (
            "builtin.shell_command",
            serde_json::json!({ "command": "echo x > out.log" }),
            ToolConcurrencyPolicy::WorkspaceCoordinated,
            ExecutionDomain::WritePaths,
        ),
        // 构建工具的写入面无法枚举：仍独占整工作区。
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

/// 写集解析失败时不能退化成"没有写入"：必须拿整工作区写闸。
#[test]
fn unparsable_edit_falls_back_to_whole_workspace() {
    let unparsable = scope(
        "builtin.edit",
        serde_json::json!({ "input": "*** Begin Patch\n*** Update File:\n*** End Patch" }),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    );
    assert_eq!(unparsable.domain, ExecutionDomain::Exclusive);
    assert!(unparsable.exclusive_workspace);
}

/// shell 的写集相对命令自身 cwd：`workdir` 不同的同名文件不能算同一条路径。
#[test]
fn shell_write_paths_are_resolved_against_the_command_cwd() {
    let in_sub = scope(
        "builtin.shell_command",
        serde_json::json!({ "command": "echo x > out.log", "workdir": "sub" }),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    );
    assert_eq!(in_sub.domain, ExecutionDomain::WritePaths);

    let edit_root_out = scope(
        "builtin.edit",
        edit_patch("out.log"),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    );
    assert_ne!(
        in_sub.write_paths(),
        edit_root_out.write_paths(),
        "workdir 下的 out.log 与会话根下的 out.log 不是同一文件"
    );

    let edit_sub_out = scope(
        "builtin.edit",
        edit_patch("sub/out.log"),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    );
    assert_eq!(
        in_sub.write_paths(),
        edit_sub_out.write_paths(),
        "workdir 下的 out.log 必须与 sub/out.log 判为同一文件"
    );
}

fn read_scope() -> ToolAccessScope {
    scope(
        "builtin.read_file",
        serde_json::json!({ "path": "src/lib.rs" }),
        ToolConcurrencyPolicy::ParallelSafe,
    )
}

fn read_other_scope() -> ToolAccessScope {
    scope(
        "builtin.read_file",
        serde_json::json!({ "path": "src/other.rs" }),
        ToolConcurrencyPolicy::ParallelSafe,
    )
}

fn write_scope() -> ToolAccessScope {
    scope(
        "builtin.edit",
        edit_patch("src/lib.rs"),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    )
}

fn write_other_scope() -> ToolAccessScope {
    scope(
        "builtin.edit",
        edit_patch("src/other.rs"),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    )
}

fn workspace_write_scope() -> ToolAccessScope {
    scope(
        "builtin.shell_command",
        serde_json::json!({ "command": "cargo test > out.log" }),
        ToolConcurrencyPolicy::WorkspaceCoordinated,
    )
}

#[test]
fn readers_share_the_gate_and_do_not_block_each_other() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(Arc::clone(&coordinator), read_scope(), read_scope(), false);
    assert_scope_blocking(
        Arc::clone(&coordinator),
        read_scope(),
        read_other_scope(),
        false,
    );
}

/// 同一路径的写撞写、写撞读、读撞写都必须串行。
#[test]
fn same_path_access_is_serialized() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(Arc::clone(&coordinator), read_scope(), write_scope(), true);
    assert_scope_blocking(Arc::clone(&coordinator), write_scope(), read_scope(), true);
    assert_scope_blocking(Arc::clone(&coordinator), write_scope(), write_scope(), true);
}

/// 不同文件之间不再互相阻塞：这就是「写与命令可以同批」的运行时依据。
#[test]
fn different_paths_can_run_concurrently() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(
        Arc::clone(&coordinator),
        write_scope(),
        write_other_scope(),
        false,
    );
    assert_scope_blocking(
        Arc::clone(&coordinator),
        write_scope(),
        read_other_scope(),
        false,
    );
    assert_scope_blocking(
        Arc::clone(&coordinator),
        read_scope(),
        write_other_scope(),
        false,
    );
}

/// 写入集无法判定的命令仍与一切读者和写者互斥。
#[test]
fn undecidable_writes_block_everything() {
    let coordinator = Arc::new(WorkspaceWriteCoordinator::default());
    assert_scope_blocking(
        Arc::clone(&coordinator),
        workspace_write_scope(),
        read_scope(),
        true,
    );
    assert_scope_blocking(
        Arc::clone(&coordinator),
        workspace_write_scope(),
        write_other_scope(),
        true,
    );
    assert_scope_blocking(
        Arc::clone(&coordinator),
        write_scope(),
        workspace_write_scope(),
        true,
    );
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
            "同路径访问必须互斥：第二个调用在观察窗口内提前进入了"
        );
    } else {
        assert!(
            entered_before_release.is_ok(),
            "不同路径不得互相阻塞：第二个调用在等待窗口内没有进入"
        );
    }
    release_tx.send(()).unwrap();
    if should_block {
        second_entered_rx.recv_timeout(READER_ENTRY_WAIT).unwrap();
    }
    first_handle.join().unwrap();
    second_handle.join().unwrap();
}
