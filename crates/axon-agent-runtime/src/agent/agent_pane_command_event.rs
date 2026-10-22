use axon_domain::PaneId;

use crate::agent_context_redaction::redact_agent_context_text;

use super::{AgentPaneCard, CommandErrorKind, CommandErrorSpan, RecentCommandState};

/// Agent Pane 命令事件来源引用前缀。
const AGENT_PANE_COMMAND_EVENT_SOURCE_PREFIX: &str = "command-event://pane/";

pub(super) fn command_event_card(
    pane_id: PaneId,
    command: &RecentCommandState,
) -> Option<AgentPaneCard> {
    recent_command_has_event(command).then(|| AgentPaneCard::CommandEvent {
        command: redact_agent_context_text(&command.command),
        exit_code: command.exit_code,
        summary: command_event_summary(command),
        source_ref: format!("{}{}", AGENT_PANE_COMMAND_EVENT_SOURCE_PREFIX, pane_id.0),
        remote_full_content_allowed: false,
    })
}

fn recent_command_has_event(command: &RecentCommandState) -> bool {
    command.exit_code.is_some_and(|code| code != 0) || !command.detected_errors.is_empty()
}

fn command_event_summary(command: &RecentCommandState) -> String {
    let exit_code = command
        .exit_code
        .map_or_else(|| "未知".to_owned(), |code| code.to_string());
    let duration = command.duration_ms.map_or_else(
        || "未知".to_owned(),
        axon_display::format_duration_millis_compact,
    );
    let mut parts = vec![format!(
        "命令 `{}` exit={} 耗时={duration}",
        command.command, exit_code
    )];
    if !command.detected_errors.is_empty() {
        parts.push(format!("错误：{}", command_error_summary(command)));
    }
    if !command.output_tail.trim().is_empty() {
        parts.push(format!("输出：{}", command.output_tail.trim()));
    }
    redact_agent_context_text(&parts.join("；"))
}

fn command_error_summary(command: &RecentCommandState) -> String {
    command
        .detected_errors
        .iter()
        .map(command_error_label)
        .collect::<Vec<_>>()
        .join(" | ")
}

fn command_error_label(error: &CommandErrorSpan) -> String {
    let location = command_error_location(error);
    if location.is_empty() {
        format!("{} {}", command_error_kind_label(error.kind), error.message)
    } else {
        format!(
            "{} {} ({location})",
            command_error_kind_label(error.kind),
            error.message
        )
    }
}

fn command_error_location(error: &CommandErrorSpan) -> String {
    let Some(file) = error
        .file
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return String::new();
    };
    match (error.line, error.column) {
        (Some(line), Some(column)) => format!("{file}:{line}:{column}"),
        (Some(line), None) => format!("{file}:{line}"),
        (None, Some(column)) => format!("{file}:?:{column}"),
        (None, None) => file.to_owned(),
    }
}

const fn command_error_kind_label(kind: CommandErrorKind) -> &'static str {
    match kind {
        CommandErrorKind::Rustc => "rustc",
        CommandErrorKind::Clang => "clang",
        CommandErrorKind::GoCompiler => "go",
        CommandErrorKind::PyTest => "pytest",
        CommandErrorKind::Eslint => "eslint",
        CommandErrorKind::TypeScript => "tsc",
        CommandErrorKind::JavaScriptTest => "js-test",
        CommandErrorKind::JavaScriptRuntime => "js-runtime",
        CommandErrorKind::Generic => "error",
    }
}
