//! Agent TUI 终端标题管理。
//!
//! 终端标题属于宿主终端的外部状态，本模块集中处理 OSC 标题写入、文本清洗与去重。

use std::fmt;
use std::io;
use std::io::IsTerminal;
use std::io::stdout;
use std::time::{Duration, Instant};

use crossterm::{Command, execute};

use crate::surface::animation::{
    AXON_ACTIVITY_LABEL, terminal_title_spinner_frame, transcript_motion_tokens,
};

const MAX_TERMINAL_TITLE_CHARS: usize = 80;
const TITLE_SEPARATOR: &str = " · ";
const DEFAULT_TERMINAL_TITLE: &str = AXON_ACTIVITY_LABEL;
const QUEUED_TERMINAL_TITLE: &str = "已排队";

/// 终端标题刷新所需的 TUI 快照。
pub(super) struct TerminalTitleContext<'a> {
    pub(super) conversation_title: Option<&'a str>,
    pub(super) workspace_name: Option<&'a str>,
    pub(super) activity: TerminalTitleActivity,
    pub(super) lifecycle_revision: u64,
}

/// 终端标题上只能同时存在一种活动态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalTitleActivity {
    Idle,
    Queued,
    Working,
}

/// 终端标题写入结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalTitleWrite {
    Written,
    Skipped,
    Empty,
}

/// 管理 Axon 写入过的终端标题。
#[derive(Debug, Default)]
pub(super) struct TerminalTitleManager {
    last_title: Option<String>,
    last_lifecycle_revision: Option<u64>,
    animation_origin: Option<Instant>,
}

impl TerminalTitleManager {
    pub(super) fn refresh(
        &mut self,
        context: &TerminalTitleContext<'_>,
        now: Instant,
    ) -> io::Result<Option<Duration>> {
        let mut stdout = stdout();
        let is_terminal = stdout.is_terminal();
        self.refresh_with_writer(context, now, &mut stdout, is_terminal)
    }

    fn refresh_with_writer(
        &mut self,
        context: &TerminalTitleContext<'_>,
        now: Instant,
        writer: &mut impl io::Write,
        is_terminal: bool,
    ) -> io::Result<Option<Duration>> {
        let animation_origin = self.animation_origin.get_or_insert(now);
        let title = build_terminal_title(context, *animation_origin, now);
        let lifecycle_changed = self.last_lifecycle_revision != Some(context.lifecycle_revision);
        if !lifecycle_changed && self.last_title.as_deref() == Some(title.as_str()) {
            return Ok(next_refresh_interval(context));
        }
        match set_terminal_title_with_writer(&title, writer, is_terminal)? {
            TerminalTitleWrite::Written | TerminalTitleWrite::Skipped => {
                self.last_title = Some(title);
                self.last_lifecycle_revision = Some(context.lifecycle_revision);
            }
            TerminalTitleWrite::Empty => {}
        }
        Ok(next_refresh_interval(context))
    }

    pub(super) fn finish(&mut self, context: &TerminalTitleContext<'_>) -> io::Result<()> {
        let mut stdout = stdout();
        let is_terminal = stdout.is_terminal();
        self.finish_with_writer(context, &mut stdout, is_terminal)
    }

    fn finish_with_writer(
        &mut self,
        context: &TerminalTitleContext<'_>,
        writer: &mut impl io::Write,
        is_terminal: bool,
    ) -> io::Result<()> {
        let title = build_stable_terminal_title(context);
        let lifecycle_changed = self.last_lifecycle_revision != Some(context.lifecycle_revision);
        if lifecycle_changed || self.last_title.as_deref() != Some(title.as_str()) {
            match set_terminal_title_with_writer(&title, writer, is_terminal)? {
                TerminalTitleWrite::Written | TerminalTitleWrite::Skipped => {
                    self.last_title = Some(title);
                    self.last_lifecycle_revision = Some(context.lifecycle_revision);
                }
                TerminalTitleWrite::Empty => {}
            }
        }
        self.animation_origin = None;
        Ok(())
    }
}

pub(super) fn finish_stable_terminal_title(context: &TerminalTitleContext<'_>) -> io::Result<()> {
    set_terminal_title(&build_stable_terminal_title(context)).map(|_| ())
}

pub(super) fn set_terminal_title(title: &str) -> io::Result<TerminalTitleWrite> {
    let mut stdout = stdout();
    let is_terminal = stdout.is_terminal();
    set_terminal_title_with_writer(title, &mut stdout, is_terminal)
}

fn set_terminal_title_with_writer(
    title: &str,
    writer: &mut impl io::Write,
    is_terminal: bool,
) -> io::Result<TerminalTitleWrite> {
    let title = sanitize_terminal_title(title);
    if title.is_empty() {
        return Ok(TerminalTitleWrite::Empty);
    }
    if !is_terminal {
        return Ok(TerminalTitleWrite::Skipped);
    }
    execute!(writer, SetWindowTitle(title))?;
    Ok(TerminalTitleWrite::Written)
}

fn build_terminal_title(
    context: &TerminalTitleContext<'_>,
    animation_origin: Instant,
    now: Instant,
) -> String {
    let mut parts = Vec::with_capacity(4);
    if let Some(activity) = activity_prefix(context, animation_origin, now) {
        parts.push(activity);
    }
    parts.push(DEFAULT_TERMINAL_TITLE.to_owned());

    // 活动态由固定品牌前缀与短标签共同承载；标题正文保持稳定，避免终端标题语义闪烁。
    if let Some(activity) = activity_label(context) {
        parts.push(activity);
    }

    // 会话标题来自当前 thread metadata；空缺时使用工作区名。
    if let Some(context_text) = extract_conversation_context(context) {
        parts.push(context_text);
    }

    parts.join(TITLE_SEPARATOR)
}

fn build_stable_terminal_title(context: &TerminalTitleContext<'_>) -> String {
    let mut parts = vec![DEFAULT_TERMINAL_TITLE.to_owned()];
    if let Some(context_text) = extract_conversation_context(context) {
        parts.push(context_text);
    }
    parts.join(TITLE_SEPARATOR)
}

fn activity_prefix(
    context: &TerminalTitleContext<'_>,
    animation_origin: Instant,
    now: Instant,
) -> Option<String> {
    matches!(context.activity, TerminalTitleActivity::Working).then(|| {
        terminal_title_spinner_frame(title_animation_frame(animation_origin, now)).to_owned()
    })
}

fn title_animation_frame(animation_origin: Instant, now: Instant) -> usize {
    let elapsed_ms = now.duration_since(animation_origin).as_millis();
    let interval_ms = transcript_motion_tokens().terminal_title.as_millis().max(1);
    (elapsed_ms / interval_ms) as usize
}

fn activity_label(context: &TerminalTitleContext<'_>) -> Option<String> {
    match context.activity {
        TerminalTitleActivity::Idle | TerminalTitleActivity::Working => None,
        TerminalTitleActivity::Queued => Some(QUEUED_TERMINAL_TITLE.to_owned()),
    }
}

fn next_refresh_interval(context: &TerminalTitleContext<'_>) -> Option<Duration> {
    matches!(context.activity, TerminalTitleActivity::Working)
        .then(|| transcript_motion_tokens().terminal_title)
}

/// 读取持久化会话标题或工作区名。
fn extract_conversation_context(context: &TerminalTitleContext<'_>) -> Option<String> {
    let raw_text = context
        .conversation_title
        .or(context.workspace_name)
        .filter(|text| !text.trim().is_empty())?;

    let normalized = raw_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '。' | '，' | ',' | '.'))
        .trim()
        .to_owned();

    if normalized.is_empty() {
        return None;
    }
    Some(normalized)
}

fn sanitize_terminal_title(title: &str) -> String {
    let mut result = String::new();
    let mut pending_space = false;
    for ch in title.chars() {
        if ch.is_whitespace() {
            pending_space = !result.is_empty();
            continue;
        }
        if is_disallowed_title_char(ch) {
            continue;
        }
        if pending_space && result.chars().count() < MAX_TERMINAL_TITLE_CHARS {
            result.push(' ');
            pending_space = false;
        }
        if result.chars().count() >= MAX_TERMINAL_TITLE_CHARS {
            break;
        }
        result.push(ch);
    }
    result.trim().to_owned()
}

fn is_disallowed_title_char(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{FE00}'..='\u{FE0F}'
                | '\u{FEFF}'
                | '\u{FFF9}'..='\u{FFFB}'
        )
}

#[derive(Debug, Clone)]
struct SetWindowTitle(String);

impl Command for SetWindowTitle {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b]0;{}\x07", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Err(io::Error::other("终端标题必须通过 ANSI OSC 序列写入"))
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[path = "terminal_title_tests.rs"]
mod tests;
