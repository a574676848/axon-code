use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolKind, ToolOutputChunk, ToolProgressInfo, ToolReceipt,
};

use super::{RunToolCompletionView, RunToolExecutionView, RunToolOutputChunk, RunToolProgressView};

pub(super) fn tool_output_chunk(chunk: ToolOutputChunk) -> RunToolOutputChunk {
    RunToolOutputChunk {
        part_id: chunk.part_id,
        provider_call_id: chunk.provider_call_id,
        tool_name: chunk.tool_name,
        kind: tool_kind_name(chunk.kind),
        stream: match chunk.stream {
            crate::tool_orchestrator::ToolOutputStream::Stdout => "stdout",
            crate::tool_orchestrator::ToolOutputStream::Stderr => "stderr",
        },
        sequence: chunk.sequence,
        text: chunk.text,
    }
}

pub(super) fn tool_progress_view(progress: ToolProgressInfo) -> RunToolProgressView {
    RunToolProgressView {
        part_id: progress.part_id,
        provider_call_id: progress.provider_call_id,
        tool_name: progress.tool_name,
        kind: tool_kind_name(progress.kind),
        phase: progress.phase,
        items_completed: progress.items_completed,
        items_total: progress.items_total,
        current_item: progress.current_item,
        elapsed_ms: progress.elapsed_ms,
        message: progress.message,
    }
}

pub(super) fn tool_completion_view_from_receipt(
    execution_request: &ToolExecutionRequest,
    receipt: &ToolReceipt,
    duration_ms: Option<u64>,
) -> RunToolCompletionView {
    RunToolCompletionView {
        part_id: execution_request.part_id.clone(),
        provider_call_id: receipt.provider_call_id.clone(),
        tool_name: receipt.tool_name.clone(),
        kind: tool_kind_name(execution_request.kind),
        status: receipt.status.clone(),
        summary: receipt.summary.clone(),
        metadata: receipt.metadata.clone(),
        error_type: receipt.error_type.clone(),
        filesystem_write_performed: receipt.filesystem_write_performed,
        duration_ms,
    }
}

pub(super) fn tool_execution_view_from_execution(
    execution_request: &ToolExecutionRequest,
) -> RunToolExecutionView {
    let arguments = execution_request.tool_call.canonical_input();
    RunToolExecutionView {
        request_id: execution_request.execution_request_id.0,
        part_id: execution_request.part_id.clone(),
        provider_call_id: execution_request.tool_call.id.clone(),
        tool_name: execution_request.tool_call.dispatch_name().to_owned(),
        kind: tool_kind_name(execution_request.kind),
        arguments_preview: bounded_arguments_preview(
            execution_request.tool_call.dispatch_name(),
            &arguments,
        ),
        // 模型自述目的在参数截断之前取出：再长的脚本或正文也不会把这一句话挤掉。
        intent: axon_agent_runtime::tool_call_intent(
            execution_request.tool_call.dispatch_name(),
            &arguments,
        ),
        execution_permission: execution_request.execution_permission.clone(),
    }
}

fn tool_kind_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Search => "search",
        ToolKind::ReadFile => "read_file",
        ToolKind::Glob => "glob",
        ToolKind::MemorySearch => "memory_search",
        ToolKind::MemoryWrite => "memory_write",
        ToolKind::History => "history",
        ToolKind::Notes => "notes",
        ToolKind::GetContextRemaining => "get_context_remaining",
        ToolKind::NewContext => "new_context",
        ToolKind::ListMcpResources | ToolKind::ReadMcpResource => "mcp_resource",
        ToolKind::UpdatePlan => "update_plan",
        ToolKind::WorkContract => "work_contract",
        ToolKind::GoalTool => "goal",
        ToolKind::Subagent => "subagent",
        ToolKind::Mcp => "mcp",
        ToolKind::InvokeSkill => "invoke_skill",
        ToolKind::Shell => "shell",
        ToolKind::Job => "job",
        ToolKind::Edit => "edit",
        ToolKind::AskUserQuestion => "ask_user_question",
        ToolKind::Unsupported => "unsupported",
    }
}

/// 便签工具的输入预览只保留模型自述的 intent，正文不进入预览投影。
fn notes_arguments_summary(tool_name: &str, arguments_json: &str) -> Option<String> {
    let name = axon_agent_runtime::bare_tool_name(tool_name);
    if !name.starts_with("notes_") {
        return None;
    }
    let intent = axon_agent_runtime::tool_call_intent(tool_name, arguments_json)?;
    Some(serde_json::json!({ "intent": intent }).to_string())
}

/// 折叠摘要：能给出可读结论时就不摆原始 JSON。
fn bounded_arguments_preview(tool_name: &str, arguments_json: &str) -> String {
    if let Some(summary) = notes_arguments_summary(tool_name, arguments_json) {
        return summary;
    }
    if let Some(summary) = question_arguments_summary(tool_name, arguments_json) {
        return summary;
    }
    // 性能优先：预览投影保持 600 字符，便签正文不进入这里。
    const MAX_CHARS: usize = 600;
    let normalized = arguments_json
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut preview = normalized.chars().take(MAX_CHARS).collect::<String>();
    if normalized.chars().count() > MAX_CHARS {
        preview.push_str("...");
    }
    preview
}

/// 询问用户工具的折叠摘要：直接用模型在调用时给出的必填摘要。
///
/// 原始参数 JSON 对用户没有信息量：行收起来时要知道的是「模型问的是什么」。
/// 摘要由模型自己写（schema 必填），这里只取用，不另行从问题正文推导。
fn question_arguments_summary(tool_name: &str, arguments_json: &str) -> Option<String> {
    if axon_agent_runtime::bare_tool_name(tool_name)
        != axon_agent_runtime::ASK_USER_QUESTION_TOOL_NAME
    {
        return None;
    }
    let parsed: serde_json::Value = serde_json::from_str(arguments_json).ok()?;
    parsed
        .get("summary")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::bounded_arguments_preview;

    #[test]
    fn notes_preview_keeps_only_intent() {
        let text = "长".repeat(500);
        let arguments = format!(
            r#"{{"intent":"固化并行开发内容的处置纪律","path":"checkpoint.md","text":"{text}"}}"#
        );
        assert_eq!(
            bounded_arguments_preview("notes_write_file", &arguments),
            r#"{"intent":"固化并行开发内容的处置纪律"}"#
        );
    }

    #[test]
    fn other_arguments_stay_bounded() {
        let arguments = "x".repeat(700);
        let preview = bounded_arguments_preview("other", &arguments);
        assert!(preview.ends_with("..."));
        assert_eq!(preview.chars().count(), 603);
    }
}
