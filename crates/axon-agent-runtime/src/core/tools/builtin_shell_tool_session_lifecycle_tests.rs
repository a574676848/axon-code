//! 终态会话生命周期的公开面回归测试。
//!
//! 覆盖模型实际能观察到的事实：终态回执仍带 `session_id`、可凭它取回终态、
//! 终态会话拒绝输入、以及「已回收」与「不存在」两种不可达原因可区分。
//! 注册表内部的墓碑语义（埋点、淘汰、过期）由 `process_manager::tombstone_tests`
//! 以注入时间确定性覆盖。

use std::path::PathBuf;

use super::{
    ShellCommandExecutionInput, ShellCommandExecutionReceipt, ShellCommandExecutionStatus,
    ShellCommandInteractionKind, ShellCommandToolRuntime, WriteStdinExecutionInput,
    process_manager,
};

/// 本测试文件全部用例的 owner。
const OWNER: &str = "session-lifecycle-tests";
/// 另一个 owner；用于验证隔离。
const OTHER_OWNER: &str = "session-lifecycle-tests-other";
/// 立即结束的命令；用于产出终态会话。
const TERMINAL_COMMAND: &str = "echo lifecycle";
/// 从未由任何会话使用过的 ID。
const NEVER_ISSUED_SESSION_ID: &str = "00000000-0000-0000-0000-000000000000";
/// 首次等待窗口；足够让立即结束的命令进入终态。
const INITIAL_YIELD_MS: u64 = 3_000;
/// 后续交互窗口。
const INTERACTION_YIELD_MS: u64 = 1_000;
/// 等待命令进入终态的最大轮询次数。
///
/// 全量测试并行运行上千个用例时，进程启动可能远超单次等待窗口；因此用
/// **有界轮询**等待终态，而不是把窗口调大了事：调大窗口只会把偶发失败推后，
/// 轮询才能消除对机器负载的依赖。
const TERMINAL_WAIT_ATTEMPTS: usize = 20;

/// 终态会话在保留窗口内仍可被工具取回，因此终态回执必须携带 `session_id`。
#[test]
fn terminal_receipt_reports_session_id() {
    let receipt = execute_terminal_command(OWNER);

    assert_eq!(receipt.status, ShellCommandExecutionStatus::Completed);
    let session_id = receipt
        .session_id
        .as_deref()
        .expect("终态回执也必须返回 session_id，否则终态输出无法通过工具取回");
    assert!(!session_id.is_empty());
}

/// 窗口内对终态会话调 `write_stdin` 返回终态回执，且连续调用幂等。
#[test]
fn terminal_session_write_stdin_returns_idempotent_terminal_receipt() {
    let manager = process_manager::ShellProcessManager::global();
    let receipt = execute_terminal_command(OWNER);
    let session_id = receipt.session_id.clone().expect("终态应带 session_id");

    let first = manager
        .write_stdin(write_input(&session_id, OWNER, None, false), None)
        .expect("终态会话应返回回执而非错误");
    let second = manager
        .write_stdin(write_input(&session_id, OWNER, None, false), None)
        .expect("终态会话应返回回执而非错误");

    assert_eq!(first.status, receipt.status);
    assert_eq!(first.exit_code, receipt.exit_code);
    assert_eq!(
        observation(&first),
        observation(&second),
        "终态会话的重复查询必须幂等"
    );
}

/// 终态会话不接受 stdin 写入，也不接受中断。
#[test]
fn terminal_session_ignores_stdin_and_interrupt() {
    let manager = process_manager::ShellProcessManager::global();
    let receipt = execute_terminal_command(OWNER);
    let session_id = receipt.session_id.clone().expect("终态应带 session_id");

    let with_stdin = manager
        .write_stdin(
            write_input(&session_id, OWNER, Some("ignored-input\n"), false),
            None,
        )
        .expect("写入请求应被安全忽略而非报错");
    let with_interrupt = manager
        .write_stdin(write_input(&session_id, OWNER, None, true), None)
        .expect("中断请求应被安全忽略而非报错");

    for (label, observed) in [("写入", with_stdin), ("中断", with_interrupt)] {
        assert_eq!(observed.status, receipt.status, "{label}后状态不得改变");
        assert_eq!(
            observed.exit_code, receipt.exit_code,
            "{label}后退出码不得改变"
        );
        assert!(!observed.cancelled, "{label}不得把已结束的会话标记为被取消");
        // B5：请求**未被施加**时回执不得宣称发生了交互。终态会话下进程一个字节
        // 都没收到，但此前 `receipt.interaction` 是无条件赋值的：TUI 会据
        // `terminalInteraction.kind` 渲染成「已交互后台终端 ＋ 输入内容」。
        assert!(
            observed.interaction.is_none(),
            "{label}请求未被施加，回执不得上报交互：{:?}",
            observed.interaction
        );
    }

    // 反向对照：**纯等待**请求（无输入、无中断）本就无「施加」可言，必须仍如实上报：
    // 缺了这条，把上报逻辑改成「恒不上报」也能让上面的断言通过。
    let wait_only = manager
        .write_stdin(write_input(&session_id, OWNER, None, false), None)
        .expect("纯等待请求应成功");
    assert_eq!(
        wait_only.interaction.map(|interaction| interaction.kind),
        Some(ShellCommandInteractionKind::Wait),
        "纯等待请求必须仍被如实上报，否则修复方向反了"
    );
}

/// 从未产生过的 ID 报告「不存在」。
#[test]
fn never_issued_session_id_reports_not_found() {
    let manager = process_manager::ShellProcessManager::global();

    let error = manager
        .write_stdin(
            write_input(NEVER_ISSUED_SESSION_ID, OWNER, None, false),
            None,
        )
        .expect_err("臆造的 session_id 必须被拒绝");

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    assert!(
        error.to_string().contains("不存在"),
        "臆造 ID 应报告「不存在」，实际为 {error}"
    );
}

/// owner 隔离在终态会话上依然生效。
#[test]
fn terminal_session_is_isolated_by_owner() {
    let manager = process_manager::ShellProcessManager::global();
    let receipt = execute_terminal_command(OWNER);
    let session_id = receipt.session_id.clone().expect("终态应带 session_id");

    let error = manager
        .write_stdin(write_input(&session_id, OTHER_OWNER, None, false), None)
        .expect_err("其他 owner 不得访问该会话");

    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

/// 工具描述必须反映三种状态，且不得再出现「非 Running 即禁止调用」的绝对化表述。
#[test]
fn shell_tool_descriptions_match_terminal_session_semantics() {
    let shell = super::shell_command_tool_definition();
    let write = super::write_stdin_tool_definition();

    for description in [&shell.description, &write.description] {
        assert!(
            !description.contains("未返回 session_id"),
            "回执始终返回 session_id，描述不得再提「未返回」：{description}"
        );
        assert!(
            !description.contains("禁止调用 write_stdin"),
            "终态会话允许查询，描述不得禁止调用：{description}"
        );
    }
    assert!(
        write.description.contains("幂等"),
        "描述应说明终态查询是幂等的：{}",
        write.description
    );
    assert!(
        axon_prompts::render_tool_usage_guidance().contains("保留窗口"),
        "会话回收属跨调用约束，必须由稳定 developer 片段承载：{}",
        axon_prompts::render_tool_usage_guidance()
    );
    assert!(
        !shell.description.contains("保留窗口"),
        "会话回收属跨调用约束，不得回流到工具 description：{}",
        shell.description
    );
}

/// 参与幂等比较的可观察字段。
fn observation(
    receipt: &ShellCommandExecutionReceipt,
) -> (String, Option<i32>, bool, bool, String) {
    (
        format!("{:?}", receipt.status),
        receipt.exit_code,
        receipt.cancelled,
        receipt.timed_out,
        format!("{}{}", receipt.stdout.preview, receipt.stderr.preview),
    )
}

/// 执行一个立即结束的命令，并有界轮询等待其进入终态。
fn execute_terminal_command(owner_id: &str) -> ShellCommandExecutionReceipt {
    let manager = process_manager::ShellProcessManager::global();
    let command = ShellCommandToolRuntime::command_from_script(TERMINAL_COMMAND, false)
        .expect("命令应规范化成功");
    let mut receipt = manager
        .execute(execution_input(command, owner_id), None)
        .expect("命令应执行成功");

    for _ in 0..TERMINAL_WAIT_ATTEMPTS {
        if receipt.status != ShellCommandExecutionStatus::Running {
            return receipt;
        }
        let session_id = receipt
            .session_id
            .clone()
            .expect("Running 会话必须返回 session_id");
        receipt = manager
            .write_stdin(write_input(&session_id, owner_id, None, false), None)
            .expect("轮询终态应成功");
    }
    panic!("命令未在有界轮询内进入终态：{receipt:?}");
}

fn execution_input(command: Vec<String>, owner_id: &str) -> ShellCommandExecutionInput {
    ShellCommandExecutionInput {
        owner_id: owner_id.to_owned(),
        command,
        cwd: std::env::current_dir().expect("测试工作目录应可解析"),
        timeout_ms: None,
        yield_time_ms: INITIAL_YIELD_MS,
        tty: false,
        run_in_background: false,
        max_output_tokens: None,
        tee_root: Some(axon_test_support::test_temp_root()),
        cancel_token: None,
        write_paths: Vec::<PathBuf>::new(),
        remote_requested: false,
    }
}

fn write_input(
    session_id: &str,
    owner_id: &str,
    chars: Option<&str>,
    ctrl_c: bool,
) -> WriteStdinExecutionInput {
    WriteStdinExecutionInput {
        owner_id: owner_id.to_owned(),
        session_id: session_id.to_owned(),
        chars: chars.map(str::to_owned),
        ctrl_c,
        yield_time_ms: INTERACTION_YIELD_MS,
        max_output_tokens: None,
        remote_requested: false,
    }
}
