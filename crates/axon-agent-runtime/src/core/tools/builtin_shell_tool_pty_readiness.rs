//! PTY 就绪与外部进程等待机制。
//!
//! 本模块是 `builtin_shell_tool_tests` 的子模块，复用父模块的测试夹具。
//!
//! # 为什么这里没有「预期耗时」常量
//!
//! 子进程启动耗时是**环境的函数**（机器速度、当前负载），不是被测系统的属性。
//! 任何写死的秒数都会同时错在两端：快机上白等，慢机或高负载下误判为失败。
//!
//! 但**加大这个秒数同样不是答案**，而且代价更隐蔽：它把正常路径的耗时也一起放大。
//! 真正的解法是换掉等待的**终止条件来源**：不再预测耗时，而是看子进程自己的状态：
//!
//! 1. 条件满足即返回；
//! 2. 会话一旦离开 `Running` 就立即返回：子进程已结束，再等没有意义，
//!    且能让断言报出「已退出」而不是含糊的「超时」；
//! 3. 墙钟只剩最后一个用途：[`HANG_GUARD`]，真挂死时的兜底。
//!
//! 由此得到一个关键性质：**守卫的取值不影响任何通过用例的耗时**。
//! 等待何时结束由子进程决定，守卫只在病态情况下才被走到：
//! 因此它取一个宽裕的值不会让测试变慢，而在原先「把秒数当预期耗时」的用法下，
//! 5 秒这个值本身就足以掐断正常路径。

#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::process::Command;
use std::{thread, time::Duration, time::Instant};

use super::*;

/// 零让出窗口轮询会话的间隔。
const SESSION_POLL_INTERVAL: Duration = Duration::from_millis(25);

/// 挂起守卫：任何等待都不得越过它。
///
/// 它**不是性能断言**，只回答「是不是真的挂死了」。正常路径不会走到它，
/// 因此取宽裕值不产生耗时代价；只有真挂死才会付出这个代价。
pub(super) const HANG_GUARD: Duration = Duration::from_secs(120);

/// 以零让出窗口轮询会话，直到条件满足、会话结束或越过挂起守卫，返回最后一次回执。
///
/// 三条退出路径含义不同，调用方应据此给出可区分的断言：
/// 条件满足 = 成功；会话不再 `Running` = 子进程已退出（**真失败**，应立即报错）；
/// 越过守卫 = 仍未就绪（**挂死或极慢**）。把后两者分开，偶发失败才可诊断。
///
/// `seed` 是调用方**已经读走**的输出。每份回执只携带上次回执之后的增量，调用方在轮询前
/// 用 `execute` 或 `write_stdin` 取走过的那一段不会再重放：不把它带进来就会永久丢掉，
/// 「输出里有某个标记」的断言会随机器快慢假失败（快机上标记在首次让出窗口内就到了）。
/// 纯状态等待传 `None`。
pub(super) fn poll_until_ready(
    session_id: &str,
    deadline: Duration,
    seed: Option<&ShellCommandExecutionReceipt>,
    mut is_ready: impl FnMut(&ShellCommandExecutionReceipt) -> bool,
) -> ShellCommandExecutionReceipt {
    let started = Instant::now();
    // 调用方已读走的增量必须作为累积起点，否则早于轮询到达的字节永远不会再出现。
    let mut stdout = seed.map_or_else(String::new, |first| first.stdout.preview.clone());
    let mut stderr = seed.map_or_else(String::new, |first| first.stderr.preview.clone());
    loop {
        let mut receipt = ShellCommandToolRuntime
            .write_stdin(write_input(session_id, None, false, 0))
            .unwrap();
        // `write_stdin` 每次只返回**增量**：必须原地累加，否则中途到达的输出会被静默丢掉。
        // 并行门禁下同一段输出常被拆成多次到达，只留最后一次的增量会让「输出里有标记」
        // 这类断言假失败（实测 stdout 全空而 exit_code 为 0）。
        stdout.push_str(&receipt.stdout.preview);
        stderr.push_str(&receipt.stderr.preview);
        receipt.stdout.preview.clone_from(&stdout);
        receipt.stderr.preview.clone_from(&stderr);
        if is_ready(&receipt)
            || receipt.status != ShellCommandExecutionStatus::Running
            || started.elapsed() >= deadline
        {
            return receipt;
        }
        thread::sleep(SESSION_POLL_INTERVAL);
    }
}

/// 等到会话离开 `Running`（子进程真的退出）为止，返回最后一次回执。
///
/// 与 [`poll_until_ready`] 共用同一套终止条件：会话一离开 `Running` 立即返回，
/// 因此 [`HANG_GUARD`] 只在真挂死时才被走到，不会拖慢任何通过用例。
///
/// 需要「等命令跑完」的用例走这里，不要用「固定窗口 × 固定次数」预测耗时：
/// 子进程启动耗时是环境的函数（机器速度、当前负载），不是被测系统的属性。
pub(super) fn poll_until_exit(session_id: &str) -> ShellCommandExecutionReceipt {
    poll_until_ready(session_id, HANG_GUARD, None, |receipt| {
        receipt.status != ShellCommandExecutionStatus::Running
    })
}

/// 等待孙进程把自己的 PID 写进约定文件。
#[cfg(windows)]
pub(super) fn wait_for_descendant_pid(path: &std::path::Path) -> Option<u32> {
    let started = Instant::now();
    while started.elapsed() < HANG_GUARD {
        if let Ok(value) = fs::read_to_string(path)
            && let Ok(pid) = value.trim().parse()
        {
            return Some(pid);
        }
        thread::sleep(SESSION_POLL_INTERVAL);
    }
    None
}

/// 等待指定进程退出；超时后仍以当前真实状态作答，不假定已退出。
#[cfg(windows)]
pub(super) fn wait_for_process_exit(pid: u32) -> bool {
    let started = Instant::now();
    while started.elapsed() < HANG_GUARD {
        if !process_is_running(pid) {
            return true;
        }
        thread::sleep(SESSION_POLL_INTERVAL);
    }
    !process_is_running(pid)
}

/// 查询进程是否存活；`tasklist` 不可用时保守判定为存活。
#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    let filter = format!("PID eq {pid}");
    let Ok(output) = Command::new("tasklist")
        .args(["/FI", &filter, "/FO", "CSV", "/NH"])
        .output()
    else {
        return true;
    };
    let expected_pid = format!("\"{pid}\"");
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split(',')
            .nth(1)
            .is_some_and(|value| value.trim() == expected_pid)
    })
}
