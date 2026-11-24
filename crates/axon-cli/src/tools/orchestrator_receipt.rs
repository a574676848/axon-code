//! 工具执行回执转换与调度回写。

use axon_agent_runtime::{
    AGENT_RUN_TOOL_ERROR_TYPE_INVALID_ARGUMENTS, ProviderToolCall, agent_run_blocked_tool_receipt,
    agent_run_completed_tool_receipt, agent_run_failed_tool_receipt,
    agent_run_failed_tool_receipt_with_error_type,
};
#[cfg(test)]
use axon_agent_runtime::{
    ProviderToolCallScheduleReport, ProviderToolCallScheduleRuntime,
    agent_run_tool_receipt_to_execution_outcome, agent_run_tool_result_receipt_from_facts,
};
use axon_domain::{AxonError, ToolError};

use super::ToolReceipt;

#[cfg(test)]
pub(crate) fn record_scheduled_execution_receipts(
    schedule: &ProviderToolCallScheduleReport,
    receipts: &[ToolReceipt],
    completed_at_ms: u64,
) -> Result<axon_agent_runtime::ProviderToolCallExecutionReport, String> {
    let outcomes = schedule
        .entries
        .iter()
        .zip(receipts.iter())
        .map(|(entry, receipt)| {
            agent_run_tool_receipt_to_execution_outcome(
                entry.tool_call_id,
                &agent_run_tool_result_receipt_from_facts(receipt),
                completed_at_ms,
            )
        })
        .collect::<Vec<_>>();

    ProviderToolCallScheduleRuntime::record_execution_receipts(schedule, outcomes)
        .map_err(|error| format!("provider tool-call execution receipt 失败：{error:?}"))
}

pub(crate) fn blocked_tool_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
) -> ToolReceipt {
    tool_receipt_from_runtime(agent_run_blocked_tool_receipt(tool_call, summary))
}

pub(crate) fn failed_tool_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
) -> ToolReceipt {
    tool_receipt_from_runtime(agent_run_failed_tool_receipt(tool_call, summary))
}

pub(crate) fn failed_tool_receipt_with_error_type(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
    error_type: &'static str,
) -> ToolReceipt {
    tool_receipt_from_runtime(agent_run_failed_tool_receipt_with_error_type(
        tool_call, summary, error_type,
    ))
}

pub(crate) fn failed_tool_input_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
) -> ToolReceipt {
    failed_tool_receipt_with_error_type(
        tool_call,
        summary,
        AGENT_RUN_TOOL_ERROR_TYPE_INVALID_ARGUMENTS,
    )
}

pub(crate) fn failed_tool_receipt_from_axon_error(
    tool_call: &ProviderToolCall,
    error: AxonError,
) -> ToolReceipt {
    match error {
        AxonError::Tool(ToolError::InvalidArguments(message)) => {
            failed_tool_input_receipt(tool_call, message)
        }
        AxonError::Tool(ToolError::ExecutionFailed(message)) => {
            failed_tool_receipt(tool_call, message)
        }
        error => failed_tool_receipt(tool_call, format!("{error:?}")),
    }
}

pub(crate) fn completed_tool_receipt(
    tool_call: &ProviderToolCall,
    summary: impl Into<String>,
    metadata: Option<serde_json::Value>,
    filesystem_write_performed: bool,
) -> ToolReceipt {
    tool_receipt_from_runtime(agent_run_completed_tool_receipt(
        tool_call,
        summary,
        metadata,
        filesystem_write_performed,
    ))
}

pub(crate) fn tool_receipt_from_runtime(
    receipt: axon_agent_runtime::AgentRunToolReceiptProjection,
) -> ToolReceipt {
    ToolReceipt {
        tool_call_id: 0,
        part_id: String::new(),
        provider_call_id: receipt.provider_call_id,
        tool_name: receipt.tool_name,
        status: receipt.status,
        summary: receipt.summary,
        metadata: receipt.metadata,
        error_type: receipt.error_type,
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: receipt.filesystem_write_performed,
        output_ref: receipt.output_ref,
        duration_ms: None,
    }
}
