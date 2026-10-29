//! 单个 Shell 进程会话的状态机：轮询、终态标记与回执组装。
//!
//! 本模块只回答「一个会话是什么、当前处于什么状态、它的回执长什么样」；
//! 会话的创建、查找、等待与回收由 `process_manager` 负责。

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use super::{
    APPROX_CHARS_PER_TOKEN, SHELL_COMMAND_CANCELLED_EXIT_CODE,
    SHELL_COMMAND_REASON_HOST_PROCESS_USED, SHELL_COMMAND_TIMEOUT_EXIT_CODE,
    ShellCommandCancelToken, ShellCommandExecutionReceipt, ShellCommandExecutionStatus,
    ShellCommandOutputObserver, WriteStdinExecutionInput, output_buffer::SharedProcessOutput,
    process_backend::ProcessBackend,
};
use crate::runtime_limits::{
    MAX_BACKGROUND_TERMINAL_WAIT_MS, MAX_SHELL_INTERACTION_YIELD_MS,
    MIN_BACKGROUND_TERMINAL_WAIT_MS, SHELL_COMMAND_CANCEL_POLL_BACKOFF_FACTOR,
    SHELL_COMMAND_CANCEL_POLL_INTERVAL_MS, SHELL_COMMAND_CANCEL_POLL_MAX_INTERVAL_MS,
    SHELL_COMMAND_OUTPUT_DRAIN_TIMEOUT_MS,
};

/// 空闲回收窗口：距 spawn 或最后一次 `write_stdin` 触碰满该时长，即判定模型已不再跟进，
/// 终止进程并投递一次回收通知（见 `session_reclaim`）。
///
/// 语义是**无轮询触碰**而不是无输出：模型每次 `write_stdin`（含纯等待）都会刷新
/// `last_activity` 从而续命，因此本窗口既是「误触交互式命令」的兜底上限，
/// 也是「模型放任不管」的回收判据。选 10 分钟：足够覆盖一次正常的轮询间隔
/// （纯等待窗口上限 5 分钟），又不会把无人看管的挂起进程留到半小时。
pub(super) const SHELL_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SHELL_REASON_PERSISTENT_SESSION: &str = "shell_command:persistent_session";
const SHELL_REASON_PTY_USED: &str = "shell_command:pty_used";
const SHELL_REASON_ARTIFACT_WRITE_FAILED: &str = "shell_command:artifact_write_failed";
const SHELL_REASON_ARTIFACT_TRUNCATED: &str = "shell_command:artifact_truncated";

pub(super) struct ShellProcessSession {
    pub(super) id: String,
    pub(super) owner_id: String,
    pub(super) command: Vec<String>,
    pub(super) cwd: PathBuf,
    pub(super) tty: bool,
    pub(super) started: Instant,
    pub(super) last_activity: Instant,
    pub(super) deadline: Option<Instant>,
    pub(super) cancel_token: Option<ShellCommandCancelToken>,
    pub(super) backend: ProcessBackend,
    pub(super) output: Arc<SharedProcessOutput>,
    pub(super) artifact_path: PathBuf,
    pub(super) terminal: Option<TerminalState>,
    /// 本次终态是否由**空闲回收**产生、且尚未通知模型。
    ///
    /// 回收通知必须只投递一次，而终结可能由回收线程、`wait_for_session` 或
    /// `write_stdin` 任一路径先行观察到；用一个一次性标记而不是「谁先看到谁通知」，
    /// 让「同一会话只通知一次」成为状态机保证。
    pub(super) reclaim_pending: bool,
    pub(super) ctrl_c_requested: bool,
    pub(super) interaction_lock: Arc<Mutex<()>>,
    pub(super) path_snapshots: Vec<(PathBuf, PathSnapshot)>,
}

impl ShellProcessSession {
    pub(super) fn poll(&mut self, now: Instant) -> std::io::Result<()> {
        if self.terminal.is_some() {
            return Ok(());
        }
        if self.cancel_token.as_ref().is_some_and(|token| token()) {
            self.backend.terminate()?;
            self.mark_terminal(SHELL_COMMAND_CANCELLED_EXIT_CODE, false, true);
        } else if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.backend.terminate()?;
            self.mark_terminal(SHELL_COMMAND_TIMEOUT_EXIT_CODE, true, false);
        } else if now.duration_since(self.last_activity) >= SHELL_IDLE_TIMEOUT {
            self.backend.terminate()?;
            self.mark_terminal(SHELL_COMMAND_CANCELLED_EXIT_CODE, false, true);
            self.reclaim_pending = true;
        } else if let Some(exit_code) = self.backend.try_wait()? {
            if self.ctrl_c_requested {
                self.mark_terminal(SHELL_COMMAND_CANCELLED_EXIT_CODE, false, true);
            } else {
                self.mark_terminal(exit_code, false, false);
            }
        }
        Ok(())
    }

    pub(super) fn mark_terminal(&mut self, exit_code: i32, timed_out: bool, cancelled: bool) {
        // 退出信号早于管道 EOF：先把读取线程排干，再让会话进入终态。
        // 顺序反了就会发出「exit_code=0 但 stdout/stderr 均为 0 字符」的回执：
        // 残余输出在回执取走之后才被 append，而终态回执只取一次。
        self.backend
            .drain_output(Duration::from_millis(SHELL_COMMAND_OUTPUT_DRAIN_TIMEOUT_MS));
        self.terminal = Some(TerminalState {
            exit_code,
            timed_out,
            cancelled,
            ended_at: Instant::now(),
        });
        self.output.notify_change();
    }

    /// 取出「空闲回收尚未通知模型」的一次性标记；已取过则返回 `false`。
    pub(super) fn take_reclaim_pending(&mut self) -> bool {
        std::mem::take(&mut self.reclaim_pending)
    }

    pub(super) fn next_check_delay(
        &self,
        now: Instant,
        cancel_poll_delay: Duration,
    ) -> Option<Duration> {
        let deadline_delay = self
            .deadline
            .map(|deadline| deadline.saturating_duration_since(now));
        let idle_delay = Some(
            SHELL_IDLE_TIMEOUT.saturating_sub(now.saturating_duration_since(self.last_activity)),
        );
        let cancel_delay = self.cancel_token.as_ref().map(|_| cancel_poll_delay);
        [deadline_delay, idle_delay, cancel_delay]
            .into_iter()
            .flatten()
            .min()
    }

    pub(super) fn receipt(&self, max_output_tokens: Option<usize>) -> ShellCommandExecutionReceipt {
        let output = self.output.take_receipt(max_output_tokens);
        let terminal = self.terminal.as_ref();
        let status = match terminal {
            None => ShellCommandExecutionStatus::Running,
            Some(state) if state.exit_code == 0 || (state.cancelled && self.ctrl_c_requested) => {
                ShellCommandExecutionStatus::Completed
            }
            Some(_) => ShellCommandExecutionStatus::Failed,
        };
        let original_output_tokens = output
            .stdout
            .original_chars
            .saturating_add(output.stderr.original_chars)
            .div_ceil(APPROX_CHARS_PER_TOKEN);
        let filtered_output_tokens = output
            .stdout
            .retained_chars
            .saturating_add(output.stderr.retained_chars)
            .div_ceil(APPROX_CHARS_PER_TOKEN);
        let artifact_failed = self.output.artifact_failed();
        let artifact_truncated = self.output.artifact_truncated();
        let reasons = shell_receipt_reasons(self.tty, artifact_failed, artifact_truncated);
        ShellCommandExecutionReceipt {
            status,
            // 终态会话在保留窗口内仍可读输出与退出码，ID 必须对模型可见；
            // 仅在 Running 时给出会让终态输出无法通过工具取回。
            session_id: Some(self.id.clone()),
            command: self.command.clone(),
            cwd: self.cwd.display().to_string(),
            exit_code: terminal.map(|state| state.exit_code),
            timed_out: terminal.is_some_and(|state| state.timed_out),
            cancelled: terminal.is_some_and(|state| state.cancelled),
            tty: self.tty,
            duration_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
            stdout: output.stdout,
            stderr: output.stderr,
            original_output_tokens,
            filtered_output_tokens,
            output_savings_pct: savings_percent(original_output_tokens, filtered_output_tokens),
            output_handler_used: "head_tail".to_owned(),
            full_output_path: Some(self.artifact_path.display().to_string()),
            artifact_truncated,
            artifact_failed,
            output_generation: self.output.change_generation(),
            changed_paths: terminal.map_or_else(Vec::new, |_| self.changed_paths()),
            remote_trigger_allowed: false,
            interaction: None,
            reasons,
        }
    }

    fn changed_paths(&self) -> Vec<PathBuf> {
        self.path_snapshots
            .iter()
            .filter(|(path, before)| PathSnapshot::capture(path) != *before)
            .map(|(path, _)| path.clone())
            .collect()
    }
}

/// 把会话已缓冲的输出事件交给观察者，返回是否曾发出事件。
pub(super) fn emit_pending_output(
    session: &Arc<Mutex<ShellProcessSession>>,
    mut observer: Option<&mut ShellCommandOutputObserver<'_>>,
) -> bool {
    let output = Arc::clone(
        &session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .output,
    );
    let mut emitted = false;
    for chunk in output.take_events() {
        emitted = true;
        if let Some(observer) = observer.as_deref_mut() {
            observer(chunk);
        }
    }
    emitted
}

#[derive(Debug, Clone, Copy)]
pub(super) enum SessionWaitKind {
    Initial,
    Background,
    Interaction,
}

/// 组装 shell 回执的审计原因。
///
/// 独立成函数以便直接覆盖归档失败与截断的各种组合，无需构造完整进程会话。
pub(super) fn shell_receipt_reasons(
    tty: bool,
    artifact_failed: bool,
    artifact_truncated: bool,
) -> Vec<String> {
    let mut reasons = vec![
        SHELL_COMMAND_REASON_HOST_PROCESS_USED.to_owned(),
        SHELL_REASON_PERSISTENT_SESSION.to_owned(),
    ];
    if tty {
        reasons.push(SHELL_REASON_PTY_USED.to_owned());
    }
    if artifact_failed {
        reasons.push(SHELL_REASON_ARTIFACT_WRITE_FAILED.to_owned());
    }
    if artifact_truncated {
        reasons.push(SHELL_REASON_ARTIFACT_TRUNCATED.to_owned());
    }
    reasons
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CancelPollBackoff {
    delay: Duration,
}

impl Default for CancelPollBackoff {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(SHELL_COMMAND_CANCEL_POLL_INTERVAL_MS),
        }
    }
}

impl CancelPollBackoff {
    pub(super) fn delay(self) -> Duration {
        self.delay
    }

    pub(super) fn reset(&mut self) {
        self.delay = Duration::from_millis(SHELL_COMMAND_CANCEL_POLL_INTERVAL_MS);
    }

    pub(super) fn increase(&mut self) {
        self.delay = self
            .delay
            .saturating_mul(SHELL_COMMAND_CANCEL_POLL_BACKOFF_FACTOR)
            .min(Duration::from_millis(
                SHELL_COMMAND_CANCEL_POLL_MAX_INTERVAL_MS,
            ));
    }
}

pub(super) fn effective_yield_duration(requested_ms: u64, kind: SessionWaitKind) -> Duration {
    let yield_ms = match kind {
        SessionWaitKind::Initial | SessionWaitKind::Interaction => {
            requested_ms.min(MAX_SHELL_INTERACTION_YIELD_MS)
        }
        SessionWaitKind::Background if requested_ms == 0 => 0,
        SessionWaitKind::Background => requested_ms.clamp(
            MIN_BACKGROUND_TERMINAL_WAIT_MS,
            MAX_BACKGROUND_TERMINAL_WAIT_MS,
        ),
    };
    Duration::from_millis(yield_ms)
}

pub(super) fn apply_terminal_interaction(
    session: &mut ShellProcessSession,
    input: &WriteStdinExecutionInput,
) -> std::io::Result<()> {
    if input.ctrl_c {
        session.ctrl_c_requested = true;
        if session.backend.ctrl_c()? {
            session.mark_terminal(SHELL_COMMAND_CANCELLED_EXIT_CODE, false, true);
        }
    } else if let Some(chars) = input.chars.as_deref().filter(|value| !value.is_empty()) {
        session.backend.write_stdin(chars.as_bytes())?;
    }
    session.last_activity = Instant::now();
    Ok(())
}

pub(super) struct TerminalState {
    pub(super) exit_code: i32,
    pub(super) timed_out: bool,
    pub(super) cancelled: bool,
    pub(super) ended_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PathSnapshot {
    exists: bool,
    content_sha256: Option<String>,
}

impl PathSnapshot {
    pub(super) fn capture(path: &Path) -> Self {
        match fs::read(path) {
            Ok(content) => Self {
                exists: true,
                content_sha256: Some(axon_crypto::sha256_hex(content)),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self {
                exists: false,
                content_sha256: None,
            },
            Err(_) => Self {
                exists: path.exists(),
                content_sha256: None,
            },
        }
    }
}

fn savings_percent(original: usize, filtered: usize) -> u8 {
    if original == 0 || filtered >= original {
        return 0;
    }
    u8::try_from(original.saturating_sub(filtered).saturating_mul(100) / original).unwrap_or(100)
}

#[cfg(test)]
mod wait_duration_tests {
    use super::*;

    #[test]
    fn background_wait_clamps_requests_to_terminal_window() {
        assert_eq!(
            effective_yield_duration(0, SessionWaitKind::Background),
            Duration::ZERO
        );
        assert_eq!(
            effective_yield_duration(1, SessionWaitKind::Background),
            Duration::from_millis(MIN_BACKGROUND_TERMINAL_WAIT_MS)
        );
        assert_eq!(
            effective_yield_duration(u64::MAX, SessionWaitKind::Background),
            Duration::from_millis(MAX_BACKGROUND_TERMINAL_WAIT_MS)
        );
    }

    #[test]
    fn foreground_and_interaction_waits_use_responsive_cap() {
        let expected = Duration::from_millis(MAX_SHELL_INTERACTION_YIELD_MS);
        assert_eq!(
            effective_yield_duration(u64::MAX, SessionWaitKind::Initial),
            expected
        );
        assert_eq!(
            effective_yield_duration(u64::MAX, SessionWaitKind::Interaction),
            expected
        );
    }

    #[test]
    fn cancellation_poll_backoff_grows_to_a_bounded_delay_and_resets() {
        let initial = Duration::from_millis(SHELL_COMMAND_CANCEL_POLL_INTERVAL_MS);
        let maximum = Duration::from_millis(SHELL_COMMAND_CANCEL_POLL_MAX_INTERVAL_MS);
        let mut backoff = CancelPollBackoff::default();

        assert_eq!(backoff.delay(), initial);
        backoff.increase();
        assert_eq!(
            backoff.delay(),
            initial.saturating_mul(SHELL_COMMAND_CANCEL_POLL_BACKOFF_FACTOR)
        );
        for _ in 0..8 {
            backoff.increase();
        }
        assert_eq!(backoff.delay(), maximum);

        backoff.reset();
        assert_eq!(backoff.delay(), initial);
    }
}
