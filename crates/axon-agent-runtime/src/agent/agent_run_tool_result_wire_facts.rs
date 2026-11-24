use std::path::PathBuf;

use crate::{
    AgentRunToolDecisionInput, AgentRunToolOutputPolicy, AgentRunToolReceiptWriteScan,
    AgentRunToolResultReceiptFacts, AgentTurnToolDecision, ProviderToolCall,
    agent_run_tool_decision, agent_run_tool_receipt_write_scan,
    agent_run_tool_result_receipts_from_facts,
};

/// Input for projecting adapter-local receipt facts into a turn-runtime decision.
pub struct AgentRunToolDecisionFactsInput<'a, Receipt> {
    /// Provider tool calls returned by the current turn.
    pub tool_calls: &'a [ProviderToolCall],
    /// Adapter-local tool receipts produced by the caller.
    pub receipts: Vec<Receipt>,
    /// Provider reasoning content carried into assistant tool-call messages.
    pub reasoning_content: Option<&'a str>,
    /// 当前模型对应的工具输出预算。
    pub output_policy: AgentRunToolOutputPolicy,
}

/// Project adapter-local receipt facts into the decision consumed by `AgentTurnRuntime`.
pub fn agent_run_tool_decision_from_facts<Receipt>(
    input: AgentRunToolDecisionFactsInput<'_, Receipt>,
) -> Result<AgentTurnToolDecision<Receipt>, String>
where
    Receipt: AgentRunToolResultReceiptFacts,
{
    let AgentRunToolDecisionFactsInput {
        tool_calls,
        receipts,
        reasoning_content,
        output_policy,
    } = input;
    let projected_receipts = agent_run_tool_result_receipts_from_facts(&receipts);
    match agent_run_tool_decision(AgentRunToolDecisionInput {
        tool_calls,
        receipts: projected_receipts,
        reasoning_content,
        output_policy,
    })? {
        AgentTurnToolDecision::Continue { next_messages, .. } => {
            Ok(AgentTurnToolDecision::Continue {
                receipts,
                next_messages,
            })
        }
        AgentTurnToolDecision::AwaitExternal { .. } => {
            Ok(AgentTurnToolDecision::AwaitExternal { receipts })
        }
    }
}

/// Return whether adapter-local receipt facts represent a completed tool call.
#[must_use]
pub fn agent_run_tool_receipt_completed_from_facts<Receipt>(receipt: &Receipt) -> bool
where
    Receipt: AgentRunToolResultReceiptFacts,
{
    receipt.status() == "completed"
}

/// Scan adapter-local receipt facts for filesystem writes and changed paths.
#[must_use]
pub fn agent_run_tool_receipt_write_scan_from_facts<Receipt>(
    receipts: &[Receipt],
) -> AgentRunToolReceiptWriteScan
where
    Receipt: AgentRunToolResultReceiptFacts,
{
    let projected_receipts = agent_run_tool_result_receipts_from_facts(receipts);
    agent_run_tool_receipt_write_scan(&projected_receipts)
}

/// Extract changed file paths from adapter-local receipt facts.
#[must_use]
pub fn agent_run_changed_file_paths_from_facts<Receipt>(receipts: &[Receipt]) -> Vec<PathBuf>
where
    Receipt: AgentRunToolResultReceiptFacts,
{
    agent_run_tool_receipt_write_scan_from_facts(receipts).changed_files
}
