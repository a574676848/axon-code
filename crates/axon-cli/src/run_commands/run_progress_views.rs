use crate::tool_orchestrator::{
    ToolExecutionRequest, ToolKind, ToolOutputChunk, ToolProgressInfo, ToolReceipt,
};

use super::{RunToolCompletionView, RunToolExecutionView, RunToolOutputChunk, RunToolProgressView};

/// 参数预览的整包展示上限：性能优先，超出后不再增长。
const ARGUMENTS_PREVIEW_MAX_CHARS: usize = 600;
/// 单个字段值的展示下限：再收缩也要留得住开头。
const ARGUMENTS_PREVIEW_VALUE_MIN_CHARS: usize = 24;
/// 单个字段值的展示上限：字段少时给足信息量，字段多时由整包上限继续收缩。
const ARGUMENTS_PREVIEW_VALUE_MAX_CHARS: usize = 400;
/// 数组在预览里最多保留的元素个数。
const ARGUMENTS_PREVIEW_MAX_ITEMS: usize = 4;
/// 预览截断标记；只影响展示，不改写内容。
const ARGUMENTS_PREVIEW_ELLIPSIS: char = '…';

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
    // 对象参数按字段值截断：半截 JSON 既不合法，也让折叠态读不出任何信息。
    if let Some(preview) = object_arguments_preview(arguments_json) {
        return preview;
    }
    // 非 JSON 参数（裸脚本、自由文本）保持单行化加截断。
    let normalized = arguments_json
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let mut preview = normalized
        .chars()
        .take(ARGUMENTS_PREVIEW_MAX_CHARS)
        .collect::<String>();
    if normalized.chars().count() > ARGUMENTS_PREVIEW_MAX_CHARS {
        preview.push_str("...");
    }
    preview
}

/// 对象参数预览：先按字段值截断，再按整包上限收缩，输出始终是合法 JSON。
///
/// 预览同时服务折叠态与展开态的输入投影，因此不能像自由文本那样从中间切断：
/// 切断后既解析不出 `intent` 这类主事实，界面上也只能摆一串半截原文。
fn object_arguments_preview(arguments_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(arguments_json.trim()).ok()?;
    let object = value.as_object()?;
    if object.is_empty() {
        return Some("{}".to_owned());
    }
    let mut budget = ARGUMENTS_PREVIEW_VALUE_MAX_CHARS;
    loop {
        let preview = render_arguments_object(object, budget);
        if preview.chars().count() <= ARGUMENTS_PREVIEW_MAX_CHARS
            || budget <= ARGUMENTS_PREVIEW_VALUE_MIN_CHARS
        {
            return Some(preview);
        }
        // 字段多或嵌套数组时逐级收缩单值预算；合法性与可读性优先于体量上限。
        budget = (budget / 2).max(ARGUMENTS_PREVIEW_VALUE_MIN_CHARS);
    }
}

/// 渲染整个参数对象；`intent` 排在首位，收缩时最后才让位。
fn render_arguments_object(
    object: &serde_json::Map<String, serde_json::Value>,
    budget: usize,
) -> String {
    let mut ordered = serde_json::Map::new();
    if let Some(intent) = object.get(axon_agent_runtime::TOOL_INTENT_FIELD) {
        ordered.insert(
            axon_agent_runtime::TOOL_INTENT_FIELD.to_owned(),
            bounded_preview_value(intent, budget),
        );
    }
    for (key, value) in object {
        if key == axon_agent_runtime::TOOL_INTENT_FIELD {
            continue;
        }
        ordered.insert(key.clone(), bounded_preview_value(value, budget));
    }
    serde_json::Value::Object(ordered).to_string()
}

/// 按字段值递归截断：字符串与数组元素只保留开头，其余标量原样保留。
fn bounded_preview_value(value: &serde_json::Value, budget: usize) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(truncate_preview_text(text, budget))
        }
        serde_json::Value::Array(items) => {
            let mut kept = Vec::new();
            let mut used = 0_usize;
            for item in items.iter().take(ARGUMENTS_PREVIEW_MAX_ITEMS) {
                let bounded = bounded_preview_value(item, budget);
                let size = serde_json::to_string(&bounded)
                    .map_or(0, |text| text.chars().count())
                    .saturating_add(1);
                if !kept.is_empty() && used.saturating_add(size) > budget {
                    break;
                }
                used = used.saturating_add(size);
                kept.push(bounded);
            }
            serde_json::Value::Array(kept)
        }
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.iter()
                .map(|(key, inner)| (key.clone(), bounded_preview_value(inner, budget)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// 截断单个字符串值并标记省略；未超限时原样返回。
fn truncate_preview_text(text: &str, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text.to_owned();
    }
    let mut truncated = text.chars().take(budget).collect::<String>();
    truncated.push(ARGUMENTS_PREVIEW_ELLIPSIS);
    truncated
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
    use super::{ARGUMENTS_PREVIEW_MAX_CHARS, bounded_arguments_preview};

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

    /// 回归背景：超长 task 被硬截断后成了非法 JSON，折叠态只能摆半截原文，
    /// 桌面端看起来像同一行被重复渲染。预览必须始终可解析。
    #[test]
    fn long_object_arguments_stay_parseable_and_keep_the_intent() {
        let arguments = serde_json::json!({
            "intent": "摸清 Axon 记忆实现",
            "run_in_background": false,
            "task": "只读调研，禁止修改任何文件。".repeat(80),
        })
        .to_string();

        let preview = bounded_arguments_preview("delegate_to_fast", &arguments);
        assert!(
            preview.chars().count() <= ARGUMENTS_PREVIEW_MAX_CHARS,
            "预览必须收敛到 {ARGUMENTS_PREVIEW_MAX_CHARS} 字符：{}",
            preview.chars().count()
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&preview).expect("预览必须是合法 JSON");
        assert_eq!(parsed["intent"], "摸清 Axon 记忆实现");
        assert_eq!(parsed["run_in_background"], false);
        let task = parsed["task"].as_str().expect("task 必须仍是字符串");
        assert!(task.ends_with('…'), "长任务只保留开头并标记截断：{task}");
    }

    #[test]
    fn nested_arguments_stay_parseable_after_shrinking() {
        let plan = (0..7)
            .map(|index| {
                serde_json::json!({
                    "step": format!("步骤{index}：{}", "可验证结果".repeat(40)),
                    "status": if index == 0 { "in_progress" } else { "pending" },
                })
            })
            .collect::<Vec<_>>();
        let arguments = serde_json::json!({
            "intent": "提交阶段快照",
            "plan": plan,
        })
        .to_string();

        let preview = bounded_arguments_preview("update_plan", &arguments);
        assert!(preview.chars().count() <= ARGUMENTS_PREVIEW_MAX_CHARS);
        let parsed: serde_json::Value =
            serde_json::from_str(&preview).expect("嵌套参数同样必须可解析");
        assert_eq!(parsed["intent"], "提交阶段快照");
        assert!(parsed["plan"].is_array(), "数组字段不得被降级成字符串");
    }
}
