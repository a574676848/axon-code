//! Runtime adapter 错误到 transcript 输出的投射。

use axon_agent_runtime::redact_agent_context_text;
use axon_protocol::wire::WireRuntimeError;

use crate::{
    r#loop::agent_tui::{
        AgentTuiRunError, RuntimeError,
        run_output::{AgentTuiRunLine, AgentTuiRunOutput},
    },
    surface::event_envelope::RuntimeEventId,
};

const WIRE_RUNTIME_ERROR_NEXT_STEP: &str =
    "运行时已保留错误上下文，可继续选择其他可用能力或检查结构化详情。";
const REPLAY_BLOCK_REASON_VISIBLE_OUTPUT: &str = "visible_output_emitted";
const REPLAY_BLOCK_REASON_NOT_SAFE: &str = "replay_not_safe";
const REPLAY_BLOCK_REASON_NONE: &str = "none";

pub(super) fn runtime_error_output(
    error: RuntimeError,
    caused_by: Option<RuntimeEventId>,
) -> AgentTuiRunOutput {
    let mut output = AgentTuiRunOutput::empty();
    output.push(runtime_error_line(&error, caused_by));
    output
}

/// 失败终态的结构化错误行。
pub(super) fn runtime_error_line(
    error: &RuntimeError,
    caused_by: Option<RuntimeEventId>,
) -> AgentTuiRunLine {
    let mut run_error = AgentTuiRunError::from_tui_error(error.to_tui_error());
    run_error = run_error.with_caused_by_runtime_event(
        caused_by
            .map(|event_id| format!("runtime-event-{}", event_id.get()))
            .unwrap_or_else(|| "runtime-finish".to_owned()),
    );
    AgentTuiRunLine::Error(run_error)
}

pub(super) fn wire_runtime_error_output(
    error: &WireRuntimeError,
    event_id: &str,
) -> AgentTuiRunOutput {
    let mut output = AgentTuiRunOutput::empty();
    output.push(AgentTuiRunLine::Error(agent_tui_run_error_from_wire(
        error, event_id,
    )));
    output
}

pub(super) fn agent_tui_run_error_from_wire(
    error: &WireRuntimeError,
    event_id: &str,
) -> AgentTuiRunError {
    let reason = redact_agent_context_text(&error.redacted_message);
    let caused_by_event = error.caused_by_event.as_deref().unwrap_or(event_id);
    let mut run_error = AgentTuiRunError::structured_with_id(
        format!("error:{event_id}"),
        reason.clone(),
        Some(error.category.clone()),
        error.http_status,
        Some(error.code.clone()),
        error.missing_field.clone(),
    )
    .with_caused_by_runtime_event(caused_by_event);
    run_error.details.push(format!("原因 {reason}"));
    if let Some(call_index) = error.call_index {
        run_error.details.push(format!("工具调用索引 {call_index}"));
    }
    run_error.next_step = Some(WIRE_RUNTIME_ERROR_NEXT_STEP.to_owned());
    run_error
}

pub(super) fn log_wire_runtime_error(scope: &str, error: &WireRuntimeError, event_id: &str) {
    let replay_block_reason = if error.replay_safe {
        REPLAY_BLOCK_REASON_NONE
    } else if error.visible_output_emitted {
        REPLAY_BLOCK_REASON_VISIBLE_OUTPUT
    } else {
        REPLAY_BLOCK_REASON_NOT_SAFE
    };
    axon_logger::error(&format!(
        "Wire runtime error: scope={scope} event_id={event_id} category={} code={} protocol={} call_index={} missing_field={} visible_output_emitted={} replay_block_reason={replay_block_reason}",
        error.category,
        error.code,
        error.provider_protocol.as_deref().unwrap_or("none"),
        error
            .call_index
            .map_or_else(|| "none".to_owned(), |value| value.to_string()),
        error.missing_field.as_deref().unwrap_or("none"),
        error.visible_output_emitted,
    ));
}

#[cfg(test)]
mod tests {
    use super::wire_runtime_error_output;
    use crate::r#loop::agent_tui::run_output::AgentTuiRunLine;

    #[test]
    fn wire_runtime_error_keeps_type_code_event_and_redacted_reason() {
        let output = wire_runtime_error_output(
            &axon_domain::RuntimeFailure::new(
                "mcp_unavailable",
                "mcp_call_failed",
                "request failed Authorization: Bearer sk-ant-test-fixture-invalid-0001",
                axon_domain::RUNTIME_FAILURE_TIMESTAMP_UNKNOWN,
            ),
            "wire-event-7",
        );
        let AgentTuiRunLine::Error(error) = &output.lines()[0] else {
            panic!("wire runtime error 应投影为结构化 Error line");
        };

        assert_eq!(error.error_type.as_deref(), Some("mcp_unavailable"));
        assert_eq!(error.error_code.as_deref(), Some("mcp_call_failed"));
        assert_eq!(
            error.caused_by_runtime_event.as_deref(),
            Some("wire-event-7")
        );
        assert!(
            error
                .details
                .iter()
                .any(|detail| detail.starts_with("原因 "))
        );
        assert!(!error.message.contains("sk-ant-test-fixture-invalid-0001"));
    }
}
