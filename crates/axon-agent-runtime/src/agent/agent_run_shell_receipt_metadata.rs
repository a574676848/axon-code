use crate::{
    ProviderToolCall, ShellCommandExecutionReceipt, ShellCommandInteraction,
    ShellCommandInteractionKind,
};

const SHELL_EXEC_KIND: &str = "exec";
const TERMINAL_INTERACTION_WAIT: &str = "wait";
const TERMINAL_INTERACTION_INPUT: &str = "input";
const TERMINAL_INTERACTION_INTERRUPT: &str = "interrupt";

pub(super) fn shell_receipt_metadata(
    tool_call: &ProviderToolCall,
    receipt: &ShellCommandExecutionReceipt,
    status: &str,
    summary: &str,
) -> serde_json::Value {
    serde_json::json!({
        "kind": SHELL_EXEC_KIND,
        "toolName": short_tool_name(tool_call.dispatch_name()),
        "status": status,
        "summary": summary,
        "argumentsPreview": receipt.command.join(" "),
        "outputPreview": shell_output_preview(receipt),
        "cwd": receipt.cwd,
        "sessionId": receipt.session_id,
        "exitCode": receipt.exit_code,
        "timedOut": receipt.timed_out,
        "cancelled": receipt.cancelled,
        "tty": receipt.tty,
        "durationMs": receipt.duration_ms,
        "changedPaths": receipt.changed_paths.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
        "output": {
            "handler": receipt.output_handler_used,
            "tokens": {
                "filtered": receipt.filtered_output_tokens,
                "original": receipt.original_output_tokens,
                "savingsPct": receipt.output_savings_pct,
            },
            "stdout": stream_metadata(&receipt.stdout),
            "stderr": stream_metadata(&receipt.stderr),
            "fullOutputAvailable": receipt.full_output_path.is_some()
                && !receipt.artifact_truncated
                && !receipt.artifact_failed,
            "artifactTruncated": receipt.artifact_truncated,
            "artifactFailed": receipt.artifact_failed,
        },
        "remoteTriggerAllowed": receipt.remote_trigger_allowed,
        "terminalInteraction": receipt.interaction.as_ref().map(terminal_interaction_metadata),
        "reasons": receipt.reasons,
    })
}

fn terminal_interaction_metadata(interaction: &ShellCommandInteraction) -> serde_json::Value {
    let kind = match interaction.kind {
        ShellCommandInteractionKind::Wait => TERMINAL_INTERACTION_WAIT,
        ShellCommandInteractionKind::Input => TERMINAL_INTERACTION_INPUT,
        ShellCommandInteractionKind::Interrupt => TERMINAL_INTERACTION_INTERRUPT,
    };
    serde_json::json!({
        "kind": kind,
        "stdin": interaction.stdin,
    })
}

pub(super) fn stream_metadata(stream: &crate::ShellCommandBoundedStream) -> serde_json::Value {
    serde_json::json!({
        "preview": stream.preview,
        "truncated": stream.truncated,
        "originalChars": stream.original_chars,
        "retainedChars": stream.retained_chars,
        "sha256": stream.sha256,
    })
}

fn shell_output_preview(receipt: &ShellCommandExecutionReceipt) -> String {
    output_preview(&receipt.stdout.preview, &receipt.stderr.preview)
}

/// 拼接两条流的可见预览；stdout 在前，空流不占位。
pub(super) fn output_preview(stdout: &str, stderr: &str) -> String {
    let stdout = stdout.trim();
    let stderr = stderr.trim();
    match (!stdout.is_empty(), !stderr.is_empty()) {
        (true, true) => format!("{stdout}\n{stderr}"),
        (true, false) => stdout.to_owned(),
        (false, true) => stderr.to_owned(),
        (false, false) => String::new(),
    }
}

pub(super) fn short_tool_name(value: &str) -> String {
    value
        .rsplit(['.', ':', '/'])
        .next()
        .unwrap_or(value)
        .trim()
        .to_owned()
}
