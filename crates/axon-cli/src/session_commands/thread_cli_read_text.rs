//! `axon debug session read` 的正文渲染。
//!
//! 单轮正文、终态标签与预览截断都在这里；`thread_cli` 只负责取数与装配，
//! 渲染细节独立成模块，避免取数逻辑与展示格式互相撑大同一个文件。

use std::fmt::Write as _;

use axon_session::{ThreadHistoryItem, Turn};

use super::super::read_notes::READ_NOTE_PREFIX;
use super::super::sha256_hex;
use super::{SESSION_READ_PREVIEW_CHARS, SessionReadReport};

pub(in crate::session_commands) fn render_read_text(report: &SessionReadReport) -> String {
    let thread = &report.thread.thread;
    let mut stdout = format!(
        "Thread: {}\n- name: {}\n- provider: {}\n- model: {}\n- cwd: {}\n- status: {:?}\n- source: {:?}\n- parent: {}\n- turns: {}\n- store: {}\n",
        thread.thread_id,
        thread.name.as_deref().unwrap_or("-"),
        thread.settings.provider,
        thread.settings.model,
        thread.settings.cwd,
        thread.status,
        thread.source,
        thread.parent_thread_id.as_deref().unwrap_or("-"),
        report.thread.turns.len(),
        report.store_path,
    );
    if let Some(compaction) = &report.thread.compacted_history {
        let _ = writeln!(
            stdout,
            "- compacted: before_tokens={} after_tokens={} through_sequence={}",
            compaction.before_tokens, compaction.after_tokens, compaction.through_sequence
        );
    }
    for (index, turn) in report.thread.turns.iter().enumerate() {
        stdout.push('\n');
        stdout.push_str(&render_turn_text(index, turn));
    }
    if report.thread.turns.is_empty() {
        stdout.push_str("\n- 正文: 没有可渲染的轮次正文");
        if !report.notes.is_empty() {
            stdout.push_str("（原因见下面的备注）");
        }
        stdout.push('\n');
    }
    for note in &report.notes {
        let _ = writeln!(stdout, "{}{note}", READ_NOTE_PREFIX);
    }
    stdout
}

/// 单轮正文。
///
/// `index` 是这一轮在 Thread 里的位置，从 0 起；`Turn.sequence` 是事件序列号（写入侧目前恒为 0），
/// 直接拿它当编号会让每一轮都显示成 `Turn 0`，因此编号只按位置来。
pub(in crate::session_commands) fn render_turn_text(index: usize, turn: &Turn) -> String {
    let user = turn_user_content(turn);
    let assistant = turn_assistant_content(turn);
    let mut text = format!(
        "Turn {} · {}\n- turn_id: {}\n- started_at_ms: {}\n- completed_at_ms: {}\n- user_sha256: {}\n- assistant_sha256: {}\n- user_chars: {}\n- assistant_chars: {}\n",
        index,
        turn_status_label(turn),
        turn.turn_id,
        turn.started_at_ms,
        turn.completed_at_ms
            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        sha256_hex(user),
        sha256_hex(assistant),
        user.chars().count(),
        assistant.chars().count(),
    );
    let _ = writeln!(
        text,
        "- user: {}",
        preview_text(user, SESSION_READ_PREVIEW_CHARS)
    );
    let _ = writeln!(
        text,
        "- assistant: {}",
        preview_text(assistant, SESSION_READ_PREVIEW_CHARS)
    );
    text
}

/// 轮次终态标签。
///
/// 用户主动中断在 Thread 里落成 `Failed` 加中断类失败载荷（`category=interrupted`），
/// 直接打印 Debug 会把「我让它停的」读成「它没做成」，因此这里把它分出来。
pub(in crate::session_commands) fn turn_status_label(turn: &Turn) -> &'static str {
    let interrupted_by_user = turn.failure.as_ref().is_some_and(|failure| {
        failure.category == axon_domain::RUNTIME_FAILURE_CATEGORY_INTERRUPTED
    });
    match (turn.status, interrupted_by_user) {
        (_, true) => "Interrupted",
        (axon_session::TurnStatus::Active, false) => "Active",
        (axon_session::TurnStatus::Completed, false) => "Completed",
        (axon_session::TurnStatus::Interrupted, false) => "Interrupted",
        (axon_session::TurnStatus::Failed, false) => "Failed",
    }
}

pub(in crate::session_commands) fn turn_user_content(turn: &Turn) -> &str {
    turn.history_items
        .iter()
        .find_map(|item| match item {
            ThreadHistoryItem::UserMessage { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

pub(in crate::session_commands) fn turn_assistant_content(turn: &Turn) -> &str {
    turn.history_items
        .iter()
        .rev()
        .find_map(|item| match item {
            ThreadHistoryItem::AssistantMessage { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .unwrap_or("")
}

fn preview_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = normalized.chars().take(max_chars).collect::<String>();
    if normalized.chars().count() > max_chars {
        preview.push_str("...");
    }
    if preview.is_empty() {
        "-".to_owned()
    } else {
        preview
    }
}
