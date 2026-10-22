//! CLI 运行结果到 TUI 终态输出的投影。
//!
//! 终态只有一个来源：Daemon 的 durable wire 事件流。这里不再消费本地 legacy turn event 通道。

use axon_agent_runtime::redact_agent_context_text;
use axon_domain::RuntimeFailure;
use axon_protocol::wire::WireEventEnvelope;
use axon_tui_runtime::{
    AgentTuiChannelAttempt, AgentTuiRunError, AgentTuiRunLine, AgentTuiRunOutput,
    AgentTuiRunSessionInfo,
};
use serde::Deserialize;

use super::{
    CliRunResult, run_output_assistant_renderer::wire_assistant_messages,
    run_output_tool_renderer::latest_wire_usage, run_output_wire_sequence::wire_sequence_lines,
};
use crate::run_commands::daemon_interaction_client::DAEMON_RUN_FAILED_EVENT;

const STDOUT_PARSE_ERROR_ID: &str = "error:stdout-json";
const STDERR_ERROR_ID: &str = "error:stderr";
const COMPLETED_JSON_ERROR_ID: &str = "error:completed-json";
const DAEMON_FAILURE_ERROR_ID: &str = "error:daemon-run";
const DAEMON_FAILURE_ERROR_TYPE: &str = "runtime";
const DAEMON_FAILURE_ERROR_CODE: &str = "daemon_run_failed";
const DAEMON_FAILURE_NEXT_STEP: &str =
    "请确认 Axon Daemon 正在运行（axon daemon status），再重试本次提交。";
const RUNTIME_ERROR_NEXT_STEP: &str =
    "运行时已保留错误上下文，可继续选择其他可用能力或检查结构化详情。";

pub(super) fn format_tui_run_output(result: &CliRunResult) -> AgentTuiRunOutput {
    let mut output = AgentTuiRunOutput::empty();
    match TuiRunJsonEvent::from_stdout(&result.stdout) {
        Ok(event) => {
            project_tui_run_event(&mut output, &event);
        }
        Err(error) if !result.stdout.trim().is_empty() => {
            let mut run_error = AgentTuiRunError::structured_with_id(
                STDOUT_PARSE_ERROR_ID,
                error.clone(),
                Some("runtime".to_owned()),
                None,
                Some("stdout_json_parse_failed".to_owned()),
                None,
            );
            run_error.details.push(format!("原因 {error}"));
            run_error.next_step = Some(RUNTIME_ERROR_NEXT_STEP.to_owned());
            output.push(AgentTuiRunLine::Error(run_error));
        }
        Err(_error) => {}
    }
    if result.exit_code != 0 && !result.stderr.trim().is_empty() {
        let reason = result.stderr.trim().to_owned();
        let mut run_error = AgentTuiRunError::structured_with_id(
            STDERR_ERROR_ID,
            reason.clone(),
            Some("runtime".to_owned()),
            None,
            Some("runtime_stderr".to_owned()),
            None,
        );
        run_error.details.push(format!("原因 {reason}"));
        run_error.next_step = Some(RUNTIME_ERROR_NEXT_STEP.to_owned());
        output.push(AgentTuiRunLine::Error(run_error));
    }
    output
}

pub(super) fn format_tui_initial_history(
    session_id: String,
    response: String,
    wire_events: &[WireEventEnvelope],
) -> AgentTuiRunOutput {
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Session(AgentTuiRunSessionInfo::new(
        session_id, None,
    )));
    for message in wire_assistant_messages(wire_events) {
        output.push(AgentTuiRunLine::Message(message));
    }
    if !output
        .lines()
        .iter()
        .any(|line| matches!(line, AgentTuiRunLine::Message(_)))
    {
        output.push(AgentTuiRunLine::Message(
            axon_tui_runtime::AgentTuiMessagePart::assistant("assistant:history", response),
        ));
    }
    if let Some(usage) = latest_wire_usage(wire_events) {
        output.push(AgentTuiRunLine::Usage(usage));
    }
    output
}

fn project_tui_run_event(output: &mut AgentTuiRunOutput, event: &TuiRunJsonEvent) {
    // 失败报告与成功报告共用同一条 durable 重放路径：实时邮箱只淘汰可重建的展示
    // 增量（见 `submission_worker_mailbox`），被淘汰的增量只能在这里按 durable 顺序
    // 完整重放补齐，因此错误行必须在重放之后收口，不能提前返回。
    let daemon_failure = daemon_failure_error(event);
    if let Some(session_id) = event
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        output.push(AgentTuiRunLine::Session(
            AgentTuiRunSessionInfo::with_title(
                session_id.to_owned(),
                event.resumed_from.clone(),
                event.session_title.clone(),
            ),
        ));
    }
    // durable sequence 是唯一顺序：wire 事件单遍投影，不再按类别重新分组。
    let wire_lines = wire_sequence_lines(&event.wire_events, &event.channel_attempts);
    let wire_runtime_error_count = wire_lines
        .iter()
        .filter(|line| matches!(line, AgentTuiRunLine::Error(_)))
        .count();
    for line in wire_lines {
        output.push(line);
    }
    if wire_runtime_error_count > 0 {
        return;
    }
    // 顶层 failure 只是 wire RuntimeError 的兜底副本；daemon 失败报告只携带失败原文本，
    // 必须显式投影为结构化错误，否则会被成功投影静默吞掉。
    if let Some(failure) = event.failure.as_ref() {
        output.push(AgentTuiRunLine::Error(run_error_from_failure(
            failure,
            COMPLETED_JSON_ERROR_ID,
            &event.channel_attempts,
        )));
        return;
    }
    if let Some(error) = daemon_failure {
        output.push(AgentTuiRunLine::Error(error));
    }
}

pub(super) fn run_error_from_failure(
    failure: &RuntimeFailure,
    event_id: &str,
    channel_attempts: &[TuiChannelAttempt],
) -> AgentTuiRunError {
    let mut error = AgentTuiRunError::structured_with_id(
        format!("error:{event_id}"),
        redact_agent_context_text(&failure.redacted_message),
        Some(failure.category.clone()),
        failure.http_status,
        Some(failure.code.clone()),
        failure.missing_field.clone(),
    )
    .with_caused_by_runtime_event(
        failure
            .caused_by_event
            .clone()
            .unwrap_or_else(|| event_id.to_owned()),
    )
    .with_channel_attempts(tui_channel_attempts(channel_attempts));
    if let Some(call_index) = failure.call_index {
        error.details.push(format!("工具调用索引 {call_index}"));
    }
    error.next_step = Some(RUNTIME_ERROR_NEXT_STEP.to_owned());
    error
}

/// Daemon 侧 `run_failed` 报告只携带失败原文本，必须显式投影为结构化错误；
/// 否则会被成功投影静默吞掉，只剩无法定位的通用失败。
fn daemon_failure_error(event: &TuiRunJsonEvent) -> Option<AgentTuiRunError> {
    if event.event.as_deref() != Some(DAEMON_RUN_FAILED_EVENT) {
        return None;
    }
    let reason = event.error.as_deref()?.trim();
    if reason.is_empty() {
        return None;
    }
    let mut error = AgentTuiRunError::structured_with_id(
        DAEMON_FAILURE_ERROR_ID,
        reason.to_owned(),
        Some(DAEMON_FAILURE_ERROR_TYPE.to_owned()),
        None,
        Some(DAEMON_FAILURE_ERROR_CODE.to_owned()),
        None,
    );
    error.details.push(format!("原因 {reason}"));
    error.next_step = Some(DAEMON_FAILURE_NEXT_STEP.to_owned());
    Some(error)
}

pub(super) fn tui_channel_attempts(attempts: &[TuiChannelAttempt]) -> Vec<AgentTuiChannelAttempt> {
    attempts
        .iter()
        .map(|attempt| AgentTuiChannelAttempt {
            provider: attempt.provider.clone(),
            model: attempt.model.clone(),
            http_status: attempt.http_status,
            outcome: attempt.outcome.clone(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TuiRunJsonEvent {
    #[serde(default)]
    event: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    resumed_from: Option<String>,
    #[serde(default)]
    session_title: Option<String>,
    #[serde(default)]
    failure: Option<RuntimeFailure>,
    #[serde(default)]
    wire_events: Vec<WireEventEnvelope>,
    #[serde(default)]
    channel_attempts: Vec<TuiChannelAttempt>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TuiChannelAttempt {
    provider: String,
    model: String,
    #[serde(default)]
    http_status: Option<u16>,
    outcome: String,
}

impl TuiRunJsonEvent {
    fn from_stdout(stdout: &str) -> Result<Self, String> {
        serde_json::from_str(stdout.trim())
            .map_err(|error| format!("TUI 内部 JSON 运行结果解析失败：{error}"))
    }
}

pub(super) fn bounded_single_line(value: &str, max_chars: usize) -> String {
    let single_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = single_line.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_owned();
    }
    let mut output = trimmed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    output.push('…');
    output
}

#[cfg(test)]
mod tests {
    use super::bounded_single_line;

    #[test]
    fn single_line_preview_keeps_title_semantics() {
        assert_eq!(bounded_single_line("标题\n第二行", 80), "标题 第二行");
    }
}
