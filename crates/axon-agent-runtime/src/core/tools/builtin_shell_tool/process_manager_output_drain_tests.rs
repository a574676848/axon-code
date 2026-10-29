//! 终态回执必须先排干输出线程再取回执的回归测试。
//!
//! 缺陷现场：回执 `status=Completed`、`exit_code=0`、`duration_ms≈3.3s`，但
//! `stdout.preview` 为空且 `original_chars=0`（stderr 同为 0），而同一命令单独执行
//! 每次都正常输出。根因是「子进程退出」早于「输出线程读到 EOF」：终态回执在残余输出
//! 被 append 之前就被取走，而终态回执只取一次，之后的输出再也没有人取。
//!
//! 本用例用测试钩子把输出线程扣在「已读到字节、尚未写进共享缓冲」处，于是
//! 「子进程已退出、输出线程尚未收口」不再是调度巧合而是确定时序：取回执时若没有
//! 等待排干，缓冲区必然为空。

use std::{sync::mpsc, thread, time::Duration};

use super::super::{ShellCommandExecutionStatus, output_readers};
use super::test_support;

/// 命令在退出前写出的唯一标记。
const MARKER: &str = "output-drain-marker";
/// 测试命令：立即写出标记并退出，退出与输出收口之间没有可依赖的间隙。
const SCRIPT: &str = "echo output-drain-marker";
/// 会话 ID。
const SESSION_ID: &str = "output-drain-tests";
/// 等待会话进入终态的最大轮询次数（含余量：全量测试并行时 spawn 可能明显变慢）。
const POLL_ATTEMPTS: usize = 3_000;
/// 轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(10);
/// 主线程取完回执后的兜底放行时间。
///
/// 正常路径由主线程取完回执后立刻放行；只有断言路径中断（例如未进入终态）才靠它
/// 解开钩子，避免把输出线程永久扣住。必须显著小于生产收口上限，否则有修复时也会
/// 因收口超时被判失败。
const FALLBACK_RELEASE: Duration = Duration::from_millis(1_500);

/// 子进程写出输出后立刻退出，终态回执仍必须带上这段输出。
#[test]
fn terminal_receipt_contains_output_written_right_before_exit() {
    // 把本次 spawn 的输出线程停在「已读到字节、尚未写进共享缓冲」处：
    // 子进程照常立即退出，于是「退出早于输出收口」成为确定时序。
    output_readers::test_hook::hold_next_reader_of_current_thread();
    let (drained_tx, drained_rx) = mpsc::channel::<()>();
    let releaser = thread::spawn(move || {
        let _ = drained_rx.recv_timeout(FALLBACK_RELEASE);
        output_readers::test_hook::release();
    });

    let session =
        test_support::running_session_with_command(SESSION_ID, std::time::Instant::now(), SCRIPT);

    let mut receipt = None;
    for _ in 0..POLL_ATTEMPTS {
        {
            let mut session = test_support::lock(&session);
            session
                .poll(std::time::Instant::now())
                .expect("轮询不应失败");
            if session.terminal.is_some() {
                // 终态回执必须**当场**取：缺陷正是「终态刚成立时缓冲还是空的」，
                // 事后再取等于把输出线程放行后的结果当成结论。
                receipt = Some(session.receipt(None));
            }
        }
        if receipt.is_some() {
            break;
        }
        thread::sleep(POLL_INTERVAL);
    }
    let _ = drained_tx.send(());
    let _ = releaser.join();

    let receipt = receipt.expect("会话应在轮询上限内进入终态");
    assert_eq!(receipt.status, ShellCommandExecutionStatus::Completed);
    assert_eq!(receipt.exit_code, Some(0));
    assert!(
        receipt.stdout.preview.contains(MARKER),
        "终态回执丢了子进程退出前写出的输出：preview={:?} original_chars={}",
        receipt.stdout.preview,
        receipt.stdout.original_chars
    );
    assert!(
        receipt.stdout.original_chars >= MARKER.chars().count(),
        "终态回执的 original_chars 必须计入退出前的输出：{}",
        receipt.stdout.original_chars
    );
}
