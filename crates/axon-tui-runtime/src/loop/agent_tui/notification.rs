//! Agent TUI 终端通知。
//!
//! 通知用于处理用户切到其他窗口时的高价值节点：长任务完成和任务失败。

use std::fmt::{self, Write};
use std::io;
use std::io::IsTerminal;
use std::io::stdout;
use std::time::{Duration, Instant};

use crossterm::{Command, execute};

use super::{AgentTuiRunOutput, AgentTuiSubmitFailure, duration_format::format_elapsed};

const LONG_TASK_NOTIFICATION_THRESHOLD: Duration = Duration::from_secs(8);
const MAX_NOTIFICATION_CHARS: usize = 120;

#[derive(Debug, Default)]
pub(super) struct TuiNotificationManager {
    turn_started_at: Option<Instant>,
    last_message: Option<String>,
}

impl TuiNotificationManager {
    pub(super) fn start_turn(&mut self, now: Instant) {
        self.turn_started_at = Some(now);
    }

    pub(super) fn notify_finished(
        &mut self,
        response: &Result<AgentTuiRunOutput, AgentTuiSubmitFailure>,
        now: Instant,
    ) -> io::Result<()> {
        let mut stdout = stdout();
        let is_terminal = stdout.is_terminal();
        self.notify_finished_with_writer(
            response,
            now,
            &mut stdout,
            is_terminal,
            supports_osc9_from_env(),
        )
    }

    fn notify_finished_with_writer(
        &mut self,
        response: &Result<AgentTuiRunOutput, AgentTuiSubmitFailure>,
        now: Instant,
        writer: &mut impl io::Write,
        is_terminal: bool,
        supports_osc9: bool,
    ) -> io::Result<()> {
        let elapsed = self
            .turn_started_at
            .take()
            .map_or(Duration::ZERO, |started_at| {
                now.saturating_duration_since(started_at)
            });
        let failed = response.is_err();
        if !failed && elapsed < LONG_TASK_NOTIFICATION_THRESHOLD {
            return Ok(());
        }
        let status = if failed { "失败" } else { "完成" };
        let mut message = format!("Axon 任务{status} · 耗时={}", format_elapsed(elapsed));
        if let Ok(output) = response {
            let token_usage = output.token_usage_total();
            if token_usage.has_tokens() {
                // Invariant: writing formatted text into String is infallible.
                write!(message, " · 用量 {}", token_usage.render_total())
                    .expect("写入 String 不应失败");
            }
        }
        self.notify_once_with_writer(&message, writer, is_terminal, supports_osc9)
    }

    fn notify_once_with_writer(
        &mut self,
        message: &str,
        writer: &mut impl io::Write,
        is_terminal: bool,
        supports_osc9: bool,
    ) -> io::Result<()> {
        let message = sanitize_notification(message);
        if message.is_empty() || self.last_message.as_deref() == Some(message.as_str()) {
            return Ok(());
        }
        post_notification_with_writer(&message, writer, is_terminal, supports_osc9)?;
        self.last_message = Some(message);
        Ok(())
    }
}

fn post_notification_with_writer(
    message: &str,
    writer: &mut impl io::Write,
    is_terminal: bool,
    supports_osc9: bool,
) -> io::Result<()> {
    let message = sanitize_notification(message);
    if message.is_empty() || !is_terminal {
        return Ok(());
    }
    if supports_osc9 {
        execute!(writer, Osc9Notification(message))
    } else {
        execute!(writer, BelNotification)
    }
}

fn supports_osc9_from_env() -> bool {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let term = std::env::var("TERM").unwrap_or_default();
    let program = term_program.to_ascii_lowercase();
    let term_name = term.to_ascii_lowercase();
    program.contains("wezterm")
        || program.contains("iterm")
        || program.contains("ghostty")
        || program.contains("warp")
        || term_name.contains("xterm-kitty")
}

fn sanitize_notification(message: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for ch in message.chars() {
        if ch.is_whitespace() {
            pending_space = !result.is_empty();
            continue;
        }
        if ch.is_control() {
            continue;
        }
        if pending_space && result.chars().count() < MAX_NOTIFICATION_CHARS {
            result.push(' ');
            pending_space = false;
        }
        if result.chars().count() >= MAX_NOTIFICATION_CHARS {
            break;
        }
        result.push(ch);
    }
    result
}

#[derive(Debug, Clone)]
struct Osc9Notification(String);

impl Command for Osc9Notification {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b]9;{}\x07", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other("终端通知必须通过 ANSI 序列写入"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy)]
struct BelNotification;

impl Command for BelNotification {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x07")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other("终端响铃必须通过 ANSI 序列写入"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use crossterm::Command;

    use super::*;

    #[test]
    fn notification_sanitizes_control_and_whitespace() {
        let message = sanitize_notification(" Axon\t完成\n\x1b  ");

        assert_eq!(message, "Axon 完成");
    }

    #[test]
    fn notification_formats_osc9_sequence() {
        let mut output = String::new();
        Osc9Notification("Axon 完成".to_owned())
            .write_ansi(&mut output)
            .expect("OSC 9 通知序列应可编码");

        assert_eq!(output, "\x1b]9;Axon 完成\x07");
    }

    #[test]
    fn finished_notification_uses_compact_token_units() {
        let mut output = AgentTuiRunOutput::empty();
        output.push(
            crate::r#loop::agent_tui::run_output::AgentTuiRunLine::Usage(
                crate::r#loop::agent_tui::AgentTuiUsageLine::with_id(
                    "usage:test:notification",
                    crate::r#loop::agent_tui::AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0),
                ),
            ),
        );
        let mut manager = TuiNotificationManager::default();
        let mut terminal_output = Vec::new();
        let started = Instant::now();
        manager.start_turn(started);

        manager
            .notify_finished_with_writer(
                &Ok(output),
                started + LONG_TASK_NOTIFICATION_THRESHOLD,
                &mut terminal_output,
                true,
                true,
            )
            .expect("通知应可处理");

        assert_eq!(
            manager.last_message.as_deref(),
            Some("Axon 任务完成 · 耗时=8s · 用量 5.11K")
        );
        let terminal_output = String::from_utf8(terminal_output).expect("通知输出应是 UTF-8");
        assert_eq!(
            terminal_output,
            "\x1b]9;Axon 任务完成 · 耗时=8s · 用量 5.11K\x07"
        );
    }
}
