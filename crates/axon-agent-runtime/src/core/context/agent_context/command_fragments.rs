//! Agent context 的命令事件与命令活动片段。

use axon_domain::PaneId;

use crate::agent_context::{
    AgentContextSection, AgentContextSectionPayload, CommandActivityContext, RecentCommandState,
};
use crate::{CommandErrorKind, CommandErrorSpan};

use super::sections::section;

pub(super) fn push_command_event_section(
    sections: &mut Vec<AgentContextSection>,
    pane_id: PaneId,
    history: &[RecentCommandState],
) {
    let commands = command_event_commands(history);
    if commands.is_empty() {
        return;
    }
    sections.push(section(
        AgentContextSectionPayload::CommandEvent { commands, pane_id },
        false,
    ));
}

pub(super) fn push_command_activity_section(
    sections: &mut Vec<AgentContextSection>,
    pane_id: PaneId,
    activity: Option<&CommandActivityContext>,
) {
    let Some(activity) = activity.filter(|activity| {
        !activity.remote_full_context_allowed && !activity.remote_trigger_allowed
    }) else {
        return;
    };
    sections.push(section(
        AgentContextSectionPayload::CommandActivity {
            activity: activity.clone(),
            pane_id,
        },
        false,
    ));
}

fn command_event_commands(history: &[RecentCommandState]) -> Vec<RecentCommandState> {
    history
        .iter()
        .filter(|command| command_has_event(command))
        .cloned()
        .collect()
}

pub(super) fn command_event_summary(history: &[RecentCommandState]) -> Option<String> {
    let lines = command_event_commands(history)
        .iter()
        .map(command_event_line)
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

pub(super) fn command_activity_summary(
    activity: Option<&CommandActivityContext>,
) -> Option<String> {
    let activity = activity?;
    if activity.remote_full_context_allowed || activity.remote_trigger_allowed {
        return None;
    }
    let status = command_activity_label(activity.activity);
    Some(format!(
        "status={status} cmd={} output_tail=\"{}\"",
        activity.command,
        activity.output_tail.trim()
    ))
}

const fn command_activity_label(activity: crate::CommandActivityKind) -> &'static str {
    match activity {
        crate::CommandActivityKind::Running => "running",
        crate::CommandActivityKind::Completed => "completed",
    }
}

fn command_has_event(command: &RecentCommandState) -> bool {
    command.exit_code.is_some_and(|code| code != 0)
        || !command.output_tail.trim().is_empty()
        || !command.detected_errors.is_empty()
}

fn command_event_line(command: &RecentCommandState) -> String {
    let exit_code = command
        .exit_code
        .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
    let duration = command.duration_ms.map_or_else(
        || "unknown".to_owned(),
        axon_display::format_duration_millis_compact,
    );
    let errors = command
        .detected_errors
        .iter()
        .map(command_error_line)
        .collect::<Vec<_>>()
        .join(" | ");
    format!(
        "cmd={} exit={} elapsed={} output_tail=\"{}\" errors=\"{}\"",
        command.command,
        exit_code,
        duration,
        command.output_tail.trim(),
        errors
    )
}

fn command_error_line(error: &CommandErrorSpan) -> String {
    format!(
        "kind={} message=\"{}\" file={} line={} column={}",
        command_error_kind_label(error.kind),
        error.message,
        optional_text(error.file.as_deref()),
        optional_number(error.line),
        optional_number(error.column)
    )
}

const fn command_error_kind_label(kind: CommandErrorKind) -> &'static str {
    match kind {
        CommandErrorKind::Rustc => "rustc",
        CommandErrorKind::Clang => "clang",
        CommandErrorKind::GoCompiler => "go_compiler",
        CommandErrorKind::PyTest => "pytest",
        CommandErrorKind::Eslint => "eslint",
        CommandErrorKind::TypeScript => "typescript",
        CommandErrorKind::JavaScriptTest => "javascript_test",
        CommandErrorKind::JavaScriptRuntime => "javascript_runtime",
        CommandErrorKind::Generic => "generic",
    }
}

fn optional_text(value: Option<&str>) -> &str {
    value.unwrap_or("none")
}

fn optional_number(value: Option<u32>) -> String {
    value.map_or_else(|| "none".to_owned(), |value| value.to_string())
}
