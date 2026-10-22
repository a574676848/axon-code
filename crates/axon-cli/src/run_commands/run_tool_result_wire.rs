use axon_agent_runtime::{
    AgentRunToolDecisionFactsInput, AgentRunToolOutputPolicy, AgentTurnToolDecision,
    ProviderToolCall, agent_run_tool_decision_from_facts,
};

use super::run_tool_loop::RunToolReceipt;

pub(super) fn tool_decision(
    tool_calls: &[ProviderToolCall],
    receipts: Vec<RunToolReceipt>,
    reasoning_content: Option<&str>,
) -> Result<AgentTurnToolDecision<RunToolReceipt>, String> {
    agent_run_tool_decision_from_facts(AgentRunToolDecisionFactsInput {
        tool_calls,
        receipts,
        reasoning_content,
        // 固定内联预算：模型与界面读同一份裁剪结果，超大正文一律进归档。
        output_policy: AgentRunToolOutputPolicy::inline_budget(),
    })
}
