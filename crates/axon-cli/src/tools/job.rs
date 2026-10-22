//! builtin.job_output / builtin.job_kill 工具执行适配。
//!
//! 后台作业与 Shell 会话是两条**独立通道**：作业注册表在 `JobRuntime` 里，
//! 因此 `session_id` 永远不会被这里的两个工具接受，`job_id` 也永远不会被
//! `write_stdin` 接受。owner 语义与 shell 工具同源（`thread_session_id`），
//! 作业的可见范围因此与发起它的 Agent run 完全一致。

use axon_agent_runtime::{
    JOB_KILL_TOOL_NAME, JOB_OUTPUT_TOOL_NAME, JobRuntime, ProviderToolCall,
    agent_run_job_kill_tool_receipt, agent_run_job_output_tool_receipt,
    agent_run_job_tool_args_from_provider_input,
};

use crate::tool_orchestrator::{
    ToolOrchestratorContext, ToolReceipt, failed_tool_input_receipt, failed_tool_receipt,
    failed_tool_receipt_from_axon_error, tool_receipt_from_runtime,
};

/// 按调度名分派作业工具。
///
/// 工具身份取自导出的名字常量，分派层不出现裸字符串：`ToolExecutionRequest`
/// 只携带规范工具名，`ToolId` 属于工具定义侧的身份。
pub(crate) fn execute_job(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    match short_tool_name(tool_call.dispatch_name()) {
        JOB_OUTPUT_TOOL_NAME => execute_job_output(tool_call, context),
        JOB_KILL_TOOL_NAME => execute_job_kill(tool_call, context),
        name => failed_tool_receipt(tool_call, format!("未知作业工具：{name}")),
    }
}

/// 读取后台作业的增量输出与状态。
pub(crate) fn execute_job_output(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let args = match job_tool_args(tool_call) {
        Ok(args) => args,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    match JobRuntime::global().read(args.job_id, &context.thread_session_id, args.yield_time_ms) {
        Ok(output) => tool_receipt_from_runtime(agent_run_job_output_tool_receipt(
            tool_call,
            &output,
            args.max_output_tokens,
        )),
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

/// 终止后台作业。
pub(crate) fn execute_job_kill(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let args = match job_tool_args(tool_call) {
        Ok(args) => args,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    match JobRuntime::global().kill(args.job_id, &context.thread_session_id) {
        Ok(status) => tool_receipt_from_runtime(agent_run_job_kill_tool_receipt(
            tool_call,
            args.job_id,
            status,
        )),
        Err(error) => failed_tool_receipt_from_axon_error(tool_call, error),
    }
}

fn job_tool_args(
    tool_call: &ProviderToolCall,
) -> Result<axon_agent_runtime::AgentRunJobToolArgs, String> {
    agent_run_job_tool_args_from_provider_input(&tool_call.input, tool_call.dispatch_name())
}

/// 从规范工具名中取出裸工具名；`builtin.job_output` → `job_output`。
fn short_tool_name(dispatch_name: &str) -> &str {
    dispatch_name
        .rsplit('.')
        .next()
        .unwrap_or(dispatch_name)
        .trim()
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
