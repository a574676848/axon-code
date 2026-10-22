use super::RunRequest;
use super::run_execution::{RunExecutionError, RunResponse, RunToolReceipt};

fn display_model_name(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value).trim()
}

const TEXT_SUMMARY_MAX_CHARS: usize = 72;
const TOOL_STATUS_COMPLETED: &str = "completed";

pub(super) fn render_text_run_summary(request: &RunRequest, response: &RunResponse) -> String {
    let mut lines = render_run_header(request, response);
    push_if_some(
        &mut lines,
        render_tool_receipt_summary(&response.tool_receipts),
    );
    lines.push(String::new());
    lines.push("Response:".to_owned());
    lines.push(response.content.clone());
    format!("{}\n", lines.join("\n"))
}

pub(super) fn render_text_run_failure(request: &RunRequest, error: &RunExecutionError) -> String {
    let tool_receipts = error.tool_receipts.as_ref();
    let mut lines = vec![
        format!("会话 {}", request.session_id),
        format!("模型 {}", display_model_name(&request.model.id)),
        format!("错误 {} · {}", error.error_type, error),
    ];
    push_if_some(&mut lines, render_tool_receipt_summary(tool_receipts));
    push_if_some(&mut lines, render_failure_next_step(tool_receipts));
    format!("{}\n", lines.join("\n"))
}

fn render_run_header(request: &RunRequest, response: &RunResponse) -> Vec<String> {
    let turn_count = response
        .turn_events
        .iter()
        .filter(|event| {
            matches!(
                event.kind,
                axon_agent_runtime::AgentTurnEventKind::TurnStarted
            )
        })
        .count();
    let mut lines = vec![
        format!("会话 {}", request.session_id),
        format!("模型 {}", display_model_name(&response.model)),
    ];
    if turn_count > 0 {
        lines.push(format!("轮次 {turn_count}"));
    }
    lines
}

fn push_if_some(lines: &mut Vec<String>, value: Option<String>) {
    if let Some(value) = value.filter(|value| !value.trim().is_empty()) {
        lines.push(value);
    }
}

fn render_tool_receipt_summary(receipts: &[RunToolReceipt]) -> Option<String> {
    if receipts.is_empty() {
        return None;
    }
    let completed = receipts
        .iter()
        .filter(|receipt| receipt.status == TOOL_STATUS_COMPLETED)
        .count();
    let blocked = receipts
        .iter()
        .filter(|receipt| receipt.error_type.as_deref() == Some("blocked"))
        .count();
    let failed = receipts.len().saturating_sub(completed + blocked);
    let writes = receipts
        .iter()
        .filter(|receipt| receipt.filesystem_write_performed)
        .count();
    let mut parts = vec![format!("工具 {completed}/{}", receipts.len())];
    if failed > 0 {
        parts.push(format!("失败 {failed}"));
    }
    if blocked > 0 {
        parts.push(format!("阻断 {blocked}"));
    }
    if writes > 0 {
        parts.push(format!("写盘 {writes}"));
    }
    if let Some(receipt) = receipts
        .iter()
        .rev()
        .find(|receipt| receipt.status != TOOL_STATUS_COMPLETED)
        .or_else(|| receipts.last())
    {
        let summary = summarize_tool_text(&receipt.summary, TEXT_SUMMARY_MAX_CHARS);
        if summary.is_empty() || is_machine_shell_summary(&summary) {
            parts.push(receipt.tool_name.clone());
        } else {
            parts.push(format!("{} · {summary}", receipt.tool_name));
        }
    }
    Some(parts.join(" · "))
}

fn is_machine_shell_summary(summary: &str) -> bool {
    summary.starts_with("shell command=")
}

fn render_failure_next_step(receipts: &[RunToolReceipt]) -> Option<String> {
    receipts
        .iter()
        .rev()
        .find(|receipt| receipt.status != TOOL_STATUS_COMPLETED)
        .map(|receipt| format!("重试 {}", receipt.tool_name))
        .or_else(|| Some("下一步 检查 provider/network/config 后重试".to_owned()))
}

fn summarize_tool_text(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut output = trimmed.chars().take(max_chars).collect::<String>();
    output.push_str("...");
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_sections_do_not_render_low_value_labels() {
        assert_eq!(render_tool_receipt_summary(&[]), None);
    }

    #[test]
    fn machine_shell_summary_is_not_projected_to_text_output() {
        let receipt = RunToolReceipt {
            tool_call_id: 0,
            part_id: "part-shell".to_owned(),
            provider_call_id: "call-shell".to_owned(),
            tool_name: "builtin.shell_command".to_owned(),
            status: "failed".to_owned(),
            summary: "shell command=powershell -Command New-Item exit_code=1 timed_out=false stdout= stderr=error".to_owned(),
            metadata: None,
            error_type: Some("invalid_arguments".to_owned()),
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: None,
        };

        let rendered = render_tool_receipt_summary(&[receipt]).expect("工具摘要");

        assert!(rendered.contains("builtin.shell_command"));
        assert!(!rendered.contains("shell command="));
        assert!(!rendered.contains("powershell -Command"));
    }
}
