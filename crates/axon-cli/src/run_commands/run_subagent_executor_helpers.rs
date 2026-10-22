//! Provider 子代理执行器的辅助函数。

use axon_agent_runtime::agent_run_tool_receipt_completed_from_facts;

use super::{RunRequest, RunSubagentExecutionInput};
use crate::run_commands::RunStopControl;

pub(super) fn child_model_history_json(
    input: &RunSubagentExecutionInput,
) -> Result<Option<String>, String> {
    let messages = input
        .fork_context
        .messages
        .iter()
        .filter(|message| {
            !crate::runtime_prompt_fragments::is_runtime_prompt_fragment_message(message)
        })
        .cloned()
        .collect::<Vec<_>>();
    if messages.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&messages)
        .map(Some)
        .map_err(|error| format!("子代理继承历史序列化失败：{error}"))
}

pub(super) fn start_child_stop_control(request: &RunRequest) -> Result<RunStopControl, String> {
    let cancel_token = request
        .cancel_token
        .clone()
        .ok_or_else(|| "普通 coding child 缺少取消令牌".to_owned())?;
    RunStopControl::start(&request.config_path, &request.session_id, cancel_token)
}

pub(super) fn persisted_memory_write_count(
    response: &crate::run_commands::run_execution::RunResponse,
) -> usize {
    response
        .tool_receipts
        .iter()
        .filter(|receipt| {
            receipt.tool_name == "builtin.memory_write"
                && agent_run_tool_receipt_completed_from_facts(*receipt)
                && receipt
                    .metadata
                    .as_ref()
                    .and_then(|metadata| metadata.get("memoryId"))
                    .is_some_and(|memory_id| !memory_id.is_null())
        })
        .count()
}
