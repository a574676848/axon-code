//! PTY/tty 会话用例。
//!
//! 从 `builtin_shell_tool_tests.rs` 拆出：这一组用例全部建立在真实伪终端之上，断言的是
//! 终端语义（回显、Ctrl+C 投影、后代进程清理），与管道会话用例的观察点与失败诊断方式
//! 都不同；混在同一文件里既不利于定位，也已超出单文件行数硬限。

use super::*;
use std::fs;
#[cfg(windows)]
use std::path::PathBuf;
use std::time::Duration;

use super::pty_readiness::{HANG_GUARD, poll_until_exit, poll_until_ready};
#[cfg(windows)]
use super::pty_readiness::{wait_for_descendant_pid, wait_for_process_exit};

/// PTY 会话就绪标记（由 [`tty_test_command`] 的脚本写出）。
const PTY_READY_MARKER: &str = "pty-ready";
/// `Read-Host` 读入后的回显标记。
const TTY_INPUT_MARKER: &str = "tty-input:axon-input";
/// PTY 进入可中断状态的标记。
const PTY_INTERRUPT_READY_MARKER: &str = "pty-interrupt-ready";
#[cfg(windows)]
const PTY_DESCENDANT_SLEEP_SECONDS: u64 = 30;

#[test]
fn tty_ctrl_c_projects_as_cancelled_session() {
    let mut input = execution_input(interruptible_tty_command());
    input.yield_time_ms = SHORT_YIELD_MS;
    input.tty = true;
    let running = ShellCommandToolRuntime.execute(input).unwrap();
    let session_id = running.session_id.expect("PTY 睡眠命令必须保持运行");
    // PTY 就绪标记晚于首次让出窗口，以零让出窗口轮询到可中断状态。
    let running = poll_until_ready(&session_id, HANG_GUARD, |receipt| {
        receipt.stdout.preview.contains(PTY_INTERRUPT_READY_MARKER)
    });
    assert_eq!(running.status, ShellCommandExecutionStatus::Running);
    assert!(
        running.stdout.preview.contains(PTY_INTERRUPT_READY_MARKER),
        "PTY 命令未在有界时间内进入可中断状态"
    );

    let mut cancelled = ShellCommandToolRuntime
        .write_stdin(write_input(&session_id, None, true, COMPLETION_YIELD_MS))
        .unwrap();
    // Ctrl+C 之后子进程还要走自己的退出路径：仍在跑就按状态等到退出，不预测耗时。
    if cancelled.status == ShellCommandExecutionStatus::Running {
        cancelled = poll_until_exit(&session_id);
    }

    assert_eq!(cancelled.status, ShellCommandExecutionStatus::Completed);
    assert_eq!(cancelled.exit_code, Some(SHELL_COMMAND_CANCELLED_EXIT_CODE));
    assert!(cancelled.cancelled);
    assert!(cancelled.tty);
}

#[test]
fn tty_command_uses_pty_and_completes() {
    let mut input = execution_input(tty_test_command());
    input.tty = true;

    let mut receipt = ShellCommandToolRuntime.execute(input).unwrap();
    // 首次让出窗口只决定「先看到多少」：先留下首段增量，再按子进程状态等到退出。
    let mut stdout = receipt.stdout.preview.clone();
    if receipt.status == ShellCommandExecutionStatus::Running {
        let session_id = receipt
            .session_id
            .clone()
            .expect("运行中的 PTY 命令必须返回 session_id");
        // 按子进程状态等到退出：PTY 冷启动耗时随机器负载波动，不能用固定窗口预测。
        receipt = poll_until_exit(&session_id);
        stdout.push_str(&receipt.stdout.preview);
        // 终态仍可能早于输出线程收口：会话在保留窗口内可继续取增量。
        stdout.push_str(&read_stdout_until_marker(
            &session_id,
            PTY_READY_MARKER,
            &receipt,
        ));
    }

    assert_eq!(
        receipt.status,
        ShellCommandExecutionStatus::Completed,
        "{receipt:?}"
    );
    assert!(receipt.tty);
    assert!(stdout.contains(PTY_READY_MARKER), "{receipt:?}");
}

#[cfg(windows)]
#[test]
fn tty_session_accepts_read_host_input() {
    let command = ShellCommandToolRuntime::command_from_script(
        "$value = Read-Host; Write-Output \"tty-input:$value\"",
        true,
    )
    .unwrap();
    let mut input = execution_input(command);
    input.tty = true;
    input.yield_time_ms = 0;
    let running = ShellCommandToolRuntime.execute(input).unwrap();
    let session_id = running.session_id.expect("Read-Host 必须保持 PTY 会话运行");

    let typed = ShellCommandToolRuntime
        .write_stdin(write_input(
            &session_id,
            Some("axon-input".to_owned()),
            false,
            0,
        ))
        .unwrap();
    assert_eq!(typed.status, ShellCommandExecutionStatus::Running);
    // PTY 回显可能在任意一次读取里到达：逐段累加才看得到，只留最后一次会漏掉它。
    let mut stdout = typed.stdout.preview.clone();

    let mut completed = ShellCommandToolRuntime
        .write_stdin(write_input(
            &session_id,
            Some(input_newline("")),
            false,
            COMPLETION_YIELD_MS,
        ))
        .unwrap();
    stdout.push_str(&completed.stdout.preview);
    // 子进程什么时候退由它自己决定：仍在跑就按状态等到退出，守卫只兜真挂死。
    if completed.status == ShellCommandExecutionStatus::Running {
        completed = poll_until_exit(&session_id);
        stdout.push_str(&completed.stdout.preview);
    }
    stdout.push_str(&read_stdout_until_marker(
        &session_id,
        TTY_INPUT_MARKER,
        &completed,
    ));
    assert_eq!(
        completed.status,
        ShellCommandExecutionStatus::Completed,
        "{completed:?}"
    );
    assert!(stdout.contains(TTY_INPUT_MARKER), "{completed:?}");
}

#[cfg(windows)]
#[test]
fn tty_owner_cleanup_terminates_descendant_process_tree() {
    let root = temp_root("pty-process-tree");
    let pid_path = root.join("child.pid");
    fs::create_dir_all(&root).unwrap();
    let escaped_pid_path = pid_path.display().to_string().replace('\'', "''");
    let script = format!(
        concat!(
            "$child = Start-Process -PassThru -WindowStyle Hidden pwsh.exe ",
            "-ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-Command',",
            "'Start-Sleep -Seconds {}'); ",
            "$child.Id | Set-Content -NoNewline -LiteralPath '{}'; ",
            "Wait-Process -Id $child.Id"
        ),
        PTY_DESCENDANT_SLEEP_SECONDS, escaped_pid_path
    );
    let mut input =
        execution_input(ShellCommandToolRuntime::command_from_script(&script, true).unwrap());
    input.owner_id = "pty-tree-owner".to_owned();
    input.yield_time_ms = SHORT_YIELD_MS;

    let running = ShellCommandToolRuntime.execute(input).unwrap();
    assert_eq!(running.status, ShellCommandExecutionStatus::Running);
    let descendant_pid = wait_for_descendant_pid(&pid_path);
    assert_eq!(
        ShellCommandToolRuntime::terminate_sessions_for_owner("pty-tree-owner"),
        1
    );

    let descendant_pid = descendant_pid.expect("PTY 子进程应在有界时间内写入 PID");
    assert!(
        wait_for_process_exit(descendant_pid),
        "owner cleanup 后 PTY 后代进程仍在运行：{descendant_pid}"
    );
    let _ = fs::remove_dir_all(root);
}

#[cfg(windows)]
fn tty_test_command() -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        &format!("Write-Output '{PTY_READY_MARKER}'"),
        true,
    )
    .unwrap()
}

#[cfg(unix)]
fn tty_test_command() -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(&format!("printf '{PTY_READY_MARKER}\\n'"), true)
        .unwrap()
}

#[cfg(windows)]
fn interruptible_tty_command() -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        &format!("Write-Output '{PTY_INTERRUPT_READY_MARKER}'; Start-Sleep -Seconds 30"),
        true,
    )
    .unwrap()
}

#[cfg(unix)]
fn interruptible_tty_command() -> Vec<String> {
    ShellCommandToolRuntime::command_from_script(
        &format!("printf '{PTY_INTERRUPT_READY_MARKER}\\n'; sleep 30"),
        true,
    )
    .unwrap()
}

#[cfg(windows)]
fn temp_root(tag: &str) -> PathBuf {
    let root = axon_test_support::test_temp_root().join(format!(
        "axon-shell-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
