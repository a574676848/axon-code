use axon_agent_runtime::{
    AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE, InvokeSkillInput, MCP_TOOL_NAMESPACE_PREFIX,
    MCP_TOOL_PROVIDER_DELIMITER, ProviderToolCall, execute_invoke_skill_with_cwd,
    redact_agent_context_text,
};
use axon_mcp_runtime::RuntimeMcpToolCallFailureKind;
use std::time::Instant;

use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolOrchestratorContext, ToolProgressInfo, ToolReceipt, ToolRuntimeEvent,
    ToolRuntimeObserver, completed_tool_receipt, failed_tool_input_receipt, failed_tool_receipt,
    failed_tool_receipt_with_error_type,
};

pub(crate) fn execute_builtin_invoke_skill(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let value = match tool_call.input.as_json() {
        Some(value) => value,
        None => {
            return failed_tool_input_receipt(tool_call, "builtin.invoke_skill 参数必须是 JSON");
        }
    };
    let Some(skill_name) = value.get("skill_name").and_then(serde_json::Value::as_str) else {
        return failed_tool_input_receipt(tool_call, "builtin.invoke_skill 输入缺少 skill_name");
    };
    if skill_name.trim().is_empty() {
        return failed_tool_input_receipt(tool_call, "builtin.invoke_skill skill_name 不能为空");
    }
    let input = InvokeSkillInput {
        skill_name: skill_name.to_owned(),
    };
    let result = if let Some(config_path) = context.config_path.as_deref() {
        crate::runtime_tools::execute_runtime_skill_invocation(
            config_path,
            Some(&context.cwd),
            &input,
        )
        .map(|text| axon_domain::ToolOutput { text })
        .map_err(|message| format!("invoke_skill: {message}"))
    } else {
        execute_invoke_skill_with_cwd(&input, Some(&context.cwd))
            .map_err(|error| format!("{error:?}"))
    };
    match result {
        Ok(output) => {
            let tool_call = ProviderToolCall {
                name: "builtin.invoke_skill".to_owned(),
                ..tool_call.clone()
            };
            completed_tool_receipt(&tool_call, output.text, None, false)
        }
        Err(error) => failed_tool_receipt(tool_call, error),
    }
}

pub(crate) fn execute_mcp(
    request: &ToolExecutionRequest,
    context: &ToolOrchestratorContext,
    mut runtime_observer: Option<&mut ToolRuntimeObserver<'_>>,
) -> ToolReceipt {
    let tool_call = &request.tool_call;
    let Some(config_path) = context.config_path.as_deref() else {
        return failed_mcp_tool_receipt(
            tool_call,
            RuntimeMcpToolCallFailureKind::InvalidConfig,
            "MCP tool 缺少本地配置路径",
        );
    };
    let started_at = Instant::now();
    emit_mcp_progress(
        &mut runtime_observer,
        request,
        "starting",
        0,
        "MCP tool 已开始调用",
        started_at,
    );
    let execution_registry = context
        .execution_tools
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match crate::runtime_tools::execute_runtime_mcp_tool_call_with_cancel(
        config_path,
        Some(&context.cwd),
        &execution_registry,
        tool_call,
        context.cancel_token.as_ref(),
    ) {
        Ok(output) => {
            emit_mcp_progress(
                &mut runtime_observer,
                request,
                "completed",
                1,
                "MCP tool 已完成调用",
                started_at,
            );
            completed_tool_receipt(
                tool_call,
                output.summary,
                Some(output.metadata),
                output.filesystem_write_performed,
            )
        }
        Err(message) => {
            emit_mcp_progress(
                &mut runtime_observer,
                request,
                "failed",
                1,
                "MCP tool 调用失败",
                started_at,
            );
            failed_mcp_tool_receipt(tool_call, message.kind(), message.message())
        }
    }
}

/// MCP 失败回执。
///
/// 调用期失败既不回写 server 级熔断，也不重建工具注册表：模型可见工具表只由发现期故障改变，
/// 单次调用失败只影响这一次调用。
fn failed_mcp_tool_receipt(
    tool_call: &ProviderToolCall,
    kind: RuntimeMcpToolCallFailureKind,
    reason: impl AsRef<str>,
) -> ToolReceipt {
    match kind {
        RuntimeMcpToolCallFailureKind::InvalidInput => {
            failed_tool_input_receipt(tool_call, reason.as_ref())
        }
        // 本地配置错误与 server 业务拒绝都是确定性失败：fast-fail，不标记降级。
        RuntimeMcpToolCallFailureKind::InvalidConfig
        | RuntimeMcpToolCallFailureKind::ToolRejected => {
            failed_tool_receipt(tool_call, reason.as_ref())
        }
        RuntimeMcpToolCallFailureKind::Unavailable
        | RuntimeMcpToolCallFailureKind::ToolCooling
        | RuntimeMcpToolCallFailureKind::ServerCircuitOpen => {
            degraded_mcp_tool_receipt(tool_call, kind, reason.as_ref())
        }
    }
}

/// 降级失败回执：作用域不同（整台 server / 单个工具 / 本次调用），措辞与 metadata 必须区分。
fn degraded_mcp_tool_receipt(
    tool_call: &ProviderToolCall,
    kind: RuntimeMcpToolCallFailureKind,
    reason: &str,
) -> ToolReceipt {
    let (server, tool) = mcp_tool_target(&tool_call.name);
    let reason = redact_agent_context_text(reason);
    #[cfg(not(test))]
    {
        axon_logger::error(&format!(
            "MCP 工具调用失败: call_id={} server={} tool={} failure_kind={} reason={}",
            tool_call.id,
            server,
            tool,
            kind.as_str(),
            reason
        ));
    }
    let summary = match kind {
        RuntimeMcpToolCallFailureKind::ServerCircuitOpen => format!(
            "MCP {server}.{tool} 调用失败：{reason}；该 server 处于短期熔断（发现期故障），可改用 builtin 或其他健康 MCP"
        ),
        RuntimeMcpToolCallFailureKind::ToolCooling => format!(
            "MCP {server}.{tool} 暂不可用：{reason}；该工具处于短期冷却，可改用同 server 的其他工具或其他健康 MCP"
        ),
        _ => format!(
            "MCP {server}.{tool} 调用失败：{reason}；同 server 的其他工具不受影响，可改用 builtin 或其他健康 MCP"
        ),
    };
    let mut receipt = failed_tool_receipt_with_error_type(
        tool_call,
        summary,
        AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE,
    );
    receipt.metadata = Some(serde_json::json!({
        "kind": "mcp",
        "server": server,
        "tool": tool,
        "mcpTool": tool_call.name,
        "error": {
            "type": AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE,
            "message": reason,
        },
        "degraded": true,
        "degradationClass": "optional",
        "circuitOpen": kind == RuntimeMcpToolCallFailureKind::ServerCircuitOpen,
        "toolCooldown": kind == RuntimeMcpToolCallFailureKind::ToolCooling,
        "rawPayloadExposed": false,
    }));
    receipt
}

fn mcp_tool_target(qualified_name: &str) -> (&str, &str) {
    qualified_name
        .strip_prefix(MCP_TOOL_NAMESPACE_PREFIX)
        .and_then(|target| target.split_once(MCP_TOOL_PROVIDER_DELIMITER))
        .unwrap_or(("unknown", qualified_name))
}

fn emit_mcp_progress(
    runtime_observer: &mut Option<&mut ToolRuntimeObserver<'_>>,
    request: &ToolExecutionRequest,
    phase: &str,
    items_completed: u64,
    message: &str,
    started_at: Instant,
) {
    let Some(observer) = runtime_observer.as_deref_mut() else {
        return;
    };
    observer(ToolRuntimeEvent::Progress(ToolProgressInfo {
        tool_call_id: request.tool_call_id,
        part_id: request.part_id.clone(),
        provider_call_id: request.tool_call.id.clone(),
        tool_name: request.tool_call.dispatch_name().to_owned(),
        kind: request.kind,
        phase: phase.to_owned(),
        items_completed: Some(items_completed),
        items_total: Some(1),
        current_item: Some(request.tool_call.dispatch_name().to_owned()),
        elapsed_ms: Some(u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)),
        message: Some(message.to_owned()),
    }));
}

pub(crate) fn execute_list_mcp_resources(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let Some(config_path) = context.config_path.as_deref() else {
        return failed_tool_receipt(tool_call, "list_mcp_resources 缺少本地配置路径");
    };
    let value = match parse_tool_args(tool_call, "list_mcp_resources") {
        Ok(value) => value,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let (server, cursor) = match mcp_list_args(&value) {
        Ok(arguments) => arguments,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    match crate::runtime_tools::execute_runtime_mcp_list_resources(
        config_path,
        Some(&context.cwd),
        server.as_deref(),
        cursor.as_deref(),
    ) {
        Ok(output) => {
            completed_tool_receipt(tool_call, output.summary, Some(output.metadata), false)
        }
        Err(message) => failed_tool_receipt(tool_call, message),
    }
}

pub(crate) fn execute_read_mcp_resource(
    tool_call: &ProviderToolCall,
    context: &ToolOrchestratorContext,
) -> ToolReceipt {
    let Some(config_path) = context.config_path.as_deref() else {
        return failed_tool_receipt(tool_call, "read_mcp_resource 缺少本地配置路径");
    };
    let value = match parse_tool_args(tool_call, "read_mcp_resource") {
        Ok(value) => value,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let server = match required_arg(&value, "server") {
        Ok(server) => server,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    let uri = match required_arg(&value, "uri") {
        Ok(uri) => uri,
        Err(message) => return failed_tool_input_receipt(tool_call, message),
    };
    match crate::runtime_tools::execute_runtime_mcp_read_resource(
        config_path,
        Some(&context.cwd),
        &server,
        &uri,
    ) {
        Ok(output) => {
            completed_tool_receipt(tool_call, output.summary, Some(output.metadata), false)
        }
        Err(message) => failed_tool_receipt(tool_call, message),
    }
}

fn parse_tool_args(tool_call: &ProviderToolCall, name: &str) -> Result<serde_json::Value, String> {
    tool_call
        .input
        .as_json()
        .cloned()
        .ok_or_else(|| format!("{name} 参数必须是 JSON"))
}

fn optional_arg(value: &serde_json::Value, name: &str) -> Result<Option<String>, String> {
    let Some(value) = value.get(name) else {
        return Ok(None);
    };
    let Some(value) = value.as_str() else {
        return Err(format!("MCP arguments.{name} 必须是非空字符串"));
    };
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("MCP arguments.{name} 必须是非空字符串"));
    }
    Ok(Some(value.to_owned()))
}

fn required_arg(value: &serde_json::Value, name: &str) -> Result<String, String> {
    optional_arg(value, name)?.ok_or_else(|| format!("MCP arguments 缺少 {name}"))
}

fn mcp_list_args(value: &serde_json::Value) -> Result<(Option<String>, Option<String>), String> {
    let server = optional_arg(value, "server")?;
    let cursor = optional_arg(value, "cursor")?;
    if cursor.is_some() && server.is_none() {
        return Err("MCP arguments 使用 cursor 时必须同时提供 server".to_owned());
    }
    Ok((server, cursor))
}

#[cfg(test)]
#[path = "runtime_capabilities_tests.rs"]
mod tests;
