//! Headless agent run 错误投影与运行错误契约。

use super::{
    AGENT_RUN_ERROR_TYPE_CONFIGURATION, AGENT_RUN_ERROR_TYPE_INTERRUPTED,
    AGENT_RUN_ERROR_TYPE_MEMORY_PERSISTENCE, AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL,
    AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT, AGENT_RUN_ERROR_TYPE_RUNTIME, AgentRunExecutionError,
    AgentRunProviderErrorProjection,
};
use crate::PROVIDER_STREAM_ERROR_EVENT_PREFIX;
use axon_domain::{AxonError, MemoryError, ProviderError};

/// Parse a provider transport message into a structured provider error projection.
#[must_use]
pub fn agent_run_provider_error_projection(
    message: &str,
) -> Option<AgentRunProviderErrorProjection> {
    let json_payload = provider_error_json_prefix(message).trim();
    let value = serde_json::from_str::<serde_json::Value>(json_payload).ok()?;
    let error = value.get("error")?;
    Some(AgentRunProviderErrorProjection {
        error_type: error
            .get("type")
            .and_then(error_string_value)
            .map(ToOwned::to_owned),
        code: error
            .get("code")
            .and_then(error_string_value)
            .map(ToOwned::to_owned),
        param: error
            .get("param")
            .and_then(error_string_value)
            .map(ToOwned::to_owned),
        message: error
            .get("message")
            .and_then(error_string_value)
            .map_or_else(|| message.to_owned(), ToOwned::to_owned),
    })
}

/// Convert a domain/runtime error into the headless run execution error contract.
pub fn agent_run_execution_error_from_axon_error<ToolReceipt: Clone>(
    error: &axon_domain::AxonError,
    tool_receipts: &[ToolReceipt],
) -> AgentRunExecutionError<ToolReceipt> {
    if let AxonError::Provider(ProviderError::Transport {
        message,
        http_status,
        provider_event_type,
        upstream_request_id,
    }) = error
    {
        let provider_error = agent_run_provider_error_projection(message);
        return AgentRunExecutionError {
            message: provider_error
                .as_ref()
                .map_or_else(|| message.clone(), |error| error.message.clone()),
            http_status: *http_status,
            error_type: AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT,
            error_code: provider_error.as_ref().and_then(|error| error.code.clone()),
            provider_protocol: None,
            error_param: provider_error
                .as_ref()
                .and_then(|error| error.param.clone()),
            failure_kind: None,
            call_index: None,
            provider_error: provider_error.map(Box::new),
            token_usage: None,
            tool_receipts: tool_receipts.to_vec().into_boxed_slice(),
            visible_output_emitted: false,
            channel_attempts: Vec::new(),
            provider_event_type: provider_event_type.clone(),
            upstream_request_id: upstream_request_id.clone(),
            committed_side_effects: false,
        };
    }
    if let AxonError::Provider(ProviderError::InvalidResponse(invalid)) = error {
        return AgentRunExecutionError {
            message: invalid.message.clone(),
            http_status: Some(200),
            error_type: AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL,
            error_code: Some(invalid.code.to_owned()),
            provider_protocol: Some(invalid.provider_protocol.clone()),
            error_param: invalid.missing_field.map(ToOwned::to_owned),
            failure_kind: Some(invalid.failure_kind.to_owned()),
            call_index: invalid.call_index,
            provider_error: Some(Box::new(AgentRunProviderErrorProjection {
                error_type: Some(AGENT_RUN_ERROR_TYPE_PROVIDER_PROTOCOL.to_owned()),
                code: Some(invalid.code.to_owned()),
                param: invalid.missing_field.map(ToOwned::to_owned),
                message: invalid.message.clone(),
            })),
            token_usage: None,
            tool_receipts: tool_receipts.to_vec().into_boxed_slice(),
            visible_output_emitted: false,
            channel_attempts: Vec::new(),
            provider_event_type: None,
            upstream_request_id: None,
            committed_side_effects: false,
        };
    }
    if let AxonError::Memory(MemoryError::Persistence(message)) = error {
        return AgentRunExecutionError {
            message: message.clone(),
            http_status: None,
            error_type: AGENT_RUN_ERROR_TYPE_MEMORY_PERSISTENCE,
            error_code: None,
            provider_protocol: None,
            error_param: None,
            failure_kind: None,
            call_index: None,
            provider_error: None,
            token_usage: None,
            tool_receipts: tool_receipts.to_vec().into_boxed_slice(),
            visible_output_emitted: false,
            channel_attempts: Vec::new(),
            provider_event_type: None,
            upstream_request_id: None,
            committed_side_effects: false,
        };
    }
    let error_type = if matches!(error, AxonError::Config(_)) {
        AGENT_RUN_ERROR_TYPE_CONFIGURATION
    } else {
        AGENT_RUN_ERROR_TYPE_RUNTIME
    };
    AgentRunExecutionError {
        message: format!("{error:?}"),
        http_status: None,
        error_type,
        error_code: None,
        provider_protocol: None,
        error_param: None,
        failure_kind: None,
        call_index: None,
        provider_error: None,
        token_usage: None,
        tool_receipts: tool_receipts.to_vec().into_boxed_slice(),
        visible_output_emitted: false,
        channel_attempts: Vec::new(),
        provider_event_type: None,
        upstream_request_id: None,
        committed_side_effects: false,
    }
}

/// Build a headless run configuration error.
pub fn agent_run_configuration_error<ToolReceipt>(
    message: impl Into<String>,
) -> AgentRunExecutionError<ToolReceipt> {
    AgentRunExecutionError {
        message: message.into(),
        http_status: None,
        error_type: AGENT_RUN_ERROR_TYPE_CONFIGURATION,
        error_code: None,
        provider_protocol: None,
        error_param: None,
        failure_kind: None,
        call_index: None,
        provider_error: None,
        token_usage: None,
        tool_receipts: Box::new([]),
        visible_output_emitted: false,
        channel_attempts: Vec::new(),
        provider_event_type: None,
        upstream_request_id: None,
        committed_side_effects: false,
    }
}

/// Build a headless run interruption error.
pub fn agent_run_interrupted_error<ToolReceipt: Clone>(
    tool_receipts: &[ToolReceipt],
) -> AgentRunExecutionError<ToolReceipt> {
    AgentRunExecutionError {
        message: "任务已由用户中断".to_owned(),
        http_status: None,
        error_type: AGENT_RUN_ERROR_TYPE_INTERRUPTED,
        error_code: None,
        provider_protocol: None,
        error_param: None,
        failure_kind: None,
        call_index: None,
        provider_error: None,
        token_usage: None,
        tool_receipts: tool_receipts.to_vec().into_boxed_slice(),
        visible_output_emitted: false,
        channel_attempts: Vec::new(),
        provider_event_type: None,
        upstream_request_id: None,
        committed_side_effects: false,
    }
}

fn provider_error_json_prefix(message: &str) -> &str {
    let message = message
        .strip_prefix(PROVIDER_STREAM_ERROR_EVENT_PREFIX)
        .unwrap_or(message);
    message
        .split_once("；reason=")
        .map_or(message, |(json, _)| json)
}

fn error_string_value(value: &serde_json::Value) -> Option<&str> {
    match value {
        serde_json::Value::String(value) => Some(value.as_str()),
        _ => None,
    }
}
