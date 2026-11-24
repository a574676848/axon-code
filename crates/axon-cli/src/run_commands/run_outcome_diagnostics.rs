use axon_agent_runtime::AgentTurnEventKind;
use serde::Serialize;

use super::run_execution::RunResponse;

const EMPTY_ASSISTANT_MESSAGE_WARNING: &str = "Provider completed without an assistant message";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FailedToolAttemptDiagnostic {
    pub(super) tool_name: String,
    pub(super) status: String,
    pub(super) summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RunOutcomeDiagnostics {
    pub(super) status: &'static str,
    pub(super) warnings: Vec<String>,
    pub(super) failed_tool_attempts: Vec<FailedToolAttemptDiagnostic>,
    pub(super) provider_elapsed_ms: u64,
    pub(super) orchestration_elapsed_ms: u64,
    pub(super) total_elapsed_ms: u64,
}

pub(super) fn collect(
    response: &RunResponse,
    total_elapsed_ms: u64,
    persona_warnings: &[String],
) -> RunOutcomeDiagnostics {
    let provider_elapsed_ms = response
        .turn_events
        .iter()
        .filter(|event| event.kind == AgentTurnEventKind::ProviderCompleted)
        .filter_map(|event| event.duration_ms)
        .sum();
    let mut warnings = persona_warnings.to_vec();
    let mut failed_tool_attempts = Vec::new();
    let mut optional_degradation = false;
    for receipt in &response.tool_receipts {
        if receipt.status != "completed" {
            failed_tool_attempts.push(FailedToolAttemptDiagnostic {
                tool_name: receipt.tool_name.clone(),
                status: receipt.status.clone(),
                summary: receipt.summary.clone(),
            });
            optional_degradation |= is_optional_degradation(receipt.metadata.as_ref());
        }
        warnings.extend(shell_warnings(receipt.metadata.as_ref()));
    }
    if response.content.trim().is_empty() {
        warnings.push(EMPTY_ASSISTANT_MESSAGE_WARNING.to_owned());
    }
    let status = if optional_degradation {
        "degraded"
    } else {
        "success"
    };
    RunOutcomeDiagnostics {
        status,
        warnings,
        failed_tool_attempts,
        provider_elapsed_ms,
        orchestration_elapsed_ms: total_elapsed_ms.saturating_sub(provider_elapsed_ms),
        total_elapsed_ms,
    }
}

fn is_optional_degradation(metadata: Option<&serde_json::Value>) -> bool {
    metadata
        .and_then(|value| value.get("degradationClass"))
        .and_then(serde_json::Value::as_str)
        == Some("optional")
}

fn shell_warnings(metadata: Option<&serde_json::Value>) -> Vec<String> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    [
        metadata.pointer("/output/stdout/preview"),
        metadata.pointer("/output/stderr/preview"),
    ]
    .into_iter()
    .flatten()
    .filter_map(serde_json::Value::as_str)
    .flat_map(str::lines)
    .filter(|line| {
        let lower = line.to_ascii_lowercase();
        lower.contains("warning:") || lower.contains("module_typeless_package_json")
    })
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(str::to_owned)
    .collect()
}

#[cfg(test)]
mod tests {
    use axon_agent_runtime::AgentRunResponse;
    use serde_json::json;

    use super::*;
    use crate::run_commands::run_tool_loop::RunToolReceipt;

    fn response(content: &str, tool_receipts: Vec<RunToolReceipt>) -> RunResponse {
        AgentRunResponse {
            content: content.to_owned(),
            assistant_part_id: "assistant:test".to_owned(),
            http_status: 200,
            token_usage: None,
            tool_receipts,
            tool_calls: Vec::new(),
            provider: "fixture".to_owned(),
            model: "fixture-model".to_owned(),
            channel_attempts: Vec::new(),
            reasoning_messages: Vec::new(),
            turn_events: Vec::new(),
            max_tokens_truncated: false,
        }
    }

    fn receipt(optional: bool) -> RunToolReceipt {
        RunToolReceipt {
            tool_call_id: 1,
            part_id: "tool:test".to_owned(),
            provider_call_id: "call:test".to_owned(),
            tool_name: "builtin.edit".to_owned(),
            status: "failed".to_owned(),
            summary: "invalid patch".to_owned(),
            metadata: optional.then(|| json!({ "degradationClass": "optional" })),
            error_type: Some("tool_validation".to_owned()),
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
            output_ref: None,
            duration_ms: None,
        }
    }

    #[test]
    fn required_tool_failure_is_an_audit_attempt_not_a_run_failure() {
        let diagnostics = collect(&response("done", vec![receipt(false)]), 0, &[]);

        assert_eq!(diagnostics.status, "success");
        assert_eq!(diagnostics.failed_tool_attempts.len(), 1);
    }

    #[test]
    fn optional_tool_failure_degrades_the_completed_run() {
        let diagnostics = collect(&response("done", vec![receipt(true)]), 0, &[]);

        assert_eq!(diagnostics.status, "degraded");
    }

    #[test]
    fn empty_assistant_message_is_a_warning_not_a_failure() {
        let diagnostics = collect(&response("  \n", Vec::new()), 0, &[]);

        assert_eq!(diagnostics.status, "success");
        assert_eq!(
            diagnostics.warnings,
            vec![EMPTY_ASSISTANT_MESSAGE_WARNING.to_owned()]
        );
    }
}
