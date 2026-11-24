use axon_domain::{ExecutionRequestId, ToolCallId};

use crate::{
    ProviderToolCallExecutionOutcome, ProviderToolCallExecutionReceipt,
    ProviderToolCallExecutionStatus,
};

use super::{
    AgentRunProviderToolReceiptJson, AgentRunToolResultReceipt, AgentRunToolResultReceiptFacts,
    agent_run_tool_result_receipts_from_facts,
};

/// 将 headless run 工具回执投影为 provider tool-call 执行回执。
#[must_use]
pub fn agent_run_tool_receipt_to_provider_receipt(
    dispatch_order: usize,
    receipt: &AgentRunToolResultReceipt,
) -> ProviderToolCallExecutionReceipt {
    ProviderToolCallExecutionReceipt {
        dispatch_order,
        tool_call_id: ToolCallId(dispatch_order as u64 + 1),
        provider_call_id: receipt.provider_call_id.clone(),
        tool_name: receipt.tool_name.clone(),
        execution_request_id: ExecutionRequestId(dispatch_order as u64 + 1),
        status: agent_run_tool_receipt_runtime_status(receipt),
        bounded_summary: receipt.summary.clone(),
        output_ref: receipt.output_ref.clone(),
        error_type: receipt.error_type.clone(),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: receipt.filesystem_write_performed,
        completed_at_ms: 0,
    }
}

/// Project headless run receipts into stable provider receipt JSON payloads.
#[must_use]
pub fn agent_run_tool_receipts_to_provider_receipt_json(
    receipts: &[AgentRunToolResultReceipt],
) -> Vec<AgentRunProviderToolReceiptJson> {
    receipts
        .iter()
        .enumerate()
        .map(|(dispatch_order, receipt)| {
            let provider_receipt =
                agent_run_tool_receipt_to_provider_receipt(dispatch_order, receipt);
            let status = agent_run_provider_receipt_status_name(&provider_receipt).to_owned();
            AgentRunProviderToolReceiptJson {
                dispatch_order: provider_receipt.dispatch_order,
                tool_call_id: provider_receipt.tool_call_id.0,
                provider_call_id: provider_receipt.provider_call_id,
                tool_name: provider_receipt.tool_name,
                execution_request_id: provider_receipt.execution_request_id.0,
                status,
                bounded_summary: provider_receipt.bounded_summary,
                metadata: receipt.metadata.clone(),
                output_ref: receipt.output_ref.clone(),
                error_type: provider_receipt.error_type,
                remote_trigger_allowed: provider_receipt.remote_trigger_allowed,
                filesystem_write_performed: provider_receipt.filesystem_write_performed,
                completed_at_ms: provider_receipt.completed_at_ms,
            }
        })
        .collect()
}

/// Project adapter-local receipt facts into stable provider receipt JSON payloads.
#[must_use]
pub fn agent_run_tool_receipt_facts_to_provider_receipt_json<Receipt>(
    receipts: &[Receipt],
) -> Vec<AgentRunProviderToolReceiptJson>
where
    Receipt: AgentRunToolResultReceiptFacts,
{
    let projected_receipts = agent_run_tool_result_receipts_from_facts(receipts);
    agent_run_tool_receipts_to_provider_receipt_json(&projected_receipts)
}

/// 将 headless run 工具回执状态投影为 provider tool-call runtime 状态。
#[must_use]
pub fn agent_run_tool_receipt_runtime_status(
    receipt: &AgentRunToolResultReceipt,
) -> ProviderToolCallExecutionStatus {
    match receipt.status.as_str() {
        "completed" => ProviderToolCallExecutionStatus::Completed,
        _ => ProviderToolCallExecutionStatus::Failed,
    }
}

/// 将 headless run 工具回执投影为 provider tool-call execution outcome。
#[must_use]
pub fn agent_run_tool_receipt_to_execution_outcome(
    tool_call_id: ToolCallId,
    receipt: &AgentRunToolResultReceipt,
    completed_at_ms: u64,
) -> ProviderToolCallExecutionOutcome {
    ProviderToolCallExecutionOutcome {
        tool_call_id,
        status: agent_run_tool_receipt_runtime_status(receipt),
        bounded_summary: receipt.summary.clone(),
        output_ref: receipt.output_ref.clone(),
        error_type: receipt.error_type.clone(),
        remote_trigger_allowed: receipt.remote_trigger_allowed,
        filesystem_write_performed: receipt.filesystem_write_performed,
        completed_at_ms,
    }
}

/// provider tool-call 执行回执的稳定文本状态名。
#[must_use]
pub fn agent_run_provider_receipt_status_name(
    receipt: &ProviderToolCallExecutionReceipt,
) -> &'static str {
    match receipt.status {
        ProviderToolCallExecutionStatus::Completed => "completed",
        ProviderToolCallExecutionStatus::Failed
            if receipt.error_type.as_deref() == Some("blocked") =>
        {
            "blocked"
        }
        ProviderToolCallExecutionStatus::Failed => "failed",
    }
}
