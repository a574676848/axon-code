//! Prompt cache provider wire 降级。
//!
//! 从 `prompt_cache` 拆分出的 provider wire 职责：把已规划的 cache hint 降低为
//! OpenAI-compatible / Anthropic Messages 的请求消息片段。
//!
//! 拆分理由（SRP）：`prompt_cache` 负责稳定 anchor / overflow 计算，
//! 本模块只负责把 hint 落到 provider wire，两者演进节奏不同，分文件高内聚。
//!
//! 滚动双断点：末尾最近 N 条非 system 消息各打一个 cache_control 断点，
//! 上一轮 tail 断点本轮成为可命中前缀（READ），最新断点为本轮 WRITE。

use crate::agent_model_tool_history::normalize_provider_history;
use crate::agent_session_stable_context::session_stable_context_kind;
use crate::{
    AgentModelMessage, PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL,
    PROMPT_CACHE_PROVIDER_REASON_ANTHROPIC_BREAKPOINTS_APPLIED,
    PROMPT_CACHE_PROVIDER_REASON_OPENAI_KEY_APPLIED,
    PROMPT_CACHE_PROVIDER_REASON_STABLE_PREFIX_ONLY, PROMPT_CACHE_ROLLING_TAIL_BREAKPOINTS,
    PROMPT_CACHE_WIRE_REASON_BREAKPOINT_BUDGET_EXHAUSTED, PROMPT_CACHE_WIRE_REASON_HINT_APPLIED,
    PROMPT_CACHE_WIRE_REASON_SYSTEM_EMPTY, PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE,
    PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY, PromptCacheHint, PromptCacheHintTarget,
    PromptCacheProviderPolicy, PromptCacheProviderWireFormat, PromptCacheProviderWireInput,
    PromptCacheProviderWirePlan, PromptCacheProviderWireRuntime, prompt_cache_tool_wire,
    provider_tool_name,
};

const ANTHROPIC_CACHE_CONTROL_BREAKPOINT_CAP: usize = 4;

impl PromptCacheProviderWireRuntime {
    /// 将 cache hint 降低到 provider wire 请求片段。
    #[must_use]
    pub fn lower(&self, input: &PromptCacheProviderWireInput) -> PromptCacheProviderWirePlan {
        let normalized = normalize_provider_history(&input.messages, input.supports_vision);
        let messages = normalized.messages.as_ref();
        let mut plan = match input.format {
            PromptCacheProviderWireFormat::OpenAiResponses => {
                lower_openai_responses_messages(input, messages)
            }
            PromptCacheProviderWireFormat::OpenAiCompatibleChat => {
                lower_openai_messages(input, messages)
            }
            PromptCacheProviderWireFormat::AnthropicMessages => {
                lower_anthropic_messages(input, messages)
            }
        };
        if !normalized.reasons.is_empty() {
            let mut reasons = normalized.reasons;
            reasons.extend(plan.reasons);
            plan.reasons = reasons;
        }
        plan
    }
}

fn lower_openai_messages(
    input: &PromptCacheProviderWireInput,
    messages: &[AgentModelMessage],
) -> PromptCacheProviderWirePlan {
    lower_openai_wire_messages(input, messages, false)
}

fn lower_openai_responses_messages(
    input: &PromptCacheProviderWireInput,
    messages: &[AgentModelMessage],
) -> PromptCacheProviderWirePlan {
    lower_openai_wire_messages(input, messages, true)
}

fn lower_openai_wire_messages(
    input: &PromptCacheProviderWireInput,
    messages: &[AgentModelMessage],
    responses: bool,
) -> PromptCacheProviderWirePlan {
    let mut openai_messages: Vec<serde_json::Value> = messages
        .iter()
        .map(|message| {
            if responses {
                prompt_cache_tool_wire::openai_responses_message(message, &input.model_tools)
            } else {
                prompt_cache_tool_wire::openai_chat_message(message, &input.model_tools)
            }
        })
        .collect();
    if !responses
        && input
            .capabilities
            .requires_reasoning_content_on_assistant_messages
    {
        fill_missing_reasoning_content(&mut openai_messages);
    }
    PromptCacheProviderWirePlan {
        openai_messages,
        anthropic_system: Vec::new(),
        anthropic_messages: Vec::new(),
        anthropic_tools: Vec::new(),
        applied_hints: Vec::new(),
        remote_raw_payload_allowed: false,
        reasons: provider_policy_reasons(input),
    }
}

/// 思考模式强制回传推理正文时，为缺失字段的历史 assistant 消息补空串。
///
/// 真实推理正文由消息元数据投影；上游只校验字段存在性，空串不会改变语义。
fn fill_missing_reasoning_content(messages: &mut [serde_json::Value]) {
    for message in messages {
        let Some(object) = message.as_object_mut() else {
            continue;
        };
        if object.get("role").and_then(serde_json::Value::as_str) != Some("assistant")
            || object.contains_key("reasoning_content")
        {
            continue;
        }
        object.insert("reasoning_content".to_owned(), serde_json::json!(""));
    }
}

fn lower_anthropic_messages(
    input: &PromptCacheProviderWireInput,
    messages: &[AgentModelMessage],
) -> PromptCacheProviderWirePlan {
    let cache_hints = input.prompt_cache_plan.cache_hints();
    let mark_system = has_hint(&cache_hints, PromptCacheHintTarget::LastSystemPrefix);
    let mark_session_context = has_hint(
        &cache_hints,
        PromptCacheHintTarget::LastSessionStableContext,
    );
    let mark_tail = has_hint(&cache_hints, PromptCacheHintTarget::RecentMessageTail);
    let mark_tool_schema = has_hint(&cache_hints, PromptCacheHintTarget::LastToolSchema);
    let (tail_breakpoints, tail_uncacheable) =
        rolling_tail_breakpoint_indices(messages, mark_tail, &input.model_tools);
    let system_breakpoint_index = last_cacheable_system_message_index(messages, mark_system);
    let session_context_breakpoint_index =
        last_session_stable_context_message_index(messages, mark_session_context);
    let mut breakpoint_budget = AnthropicBreakpointBudget::new();
    let tool_cache_index = (mark_tool_schema
        && session_context_breakpoint_index.is_none()
        && !input.model_tools.is_empty())
    .then(|| input.model_tools.len() - 1);
    let tool_schema_should_mark = tool_cache_index.is_some() && breakpoint_budget.try_take();
    let mut applied_hints = Vec::new();
    let mut reasons = PromptCacheWireReasonCollector::default();
    if tail_uncacheable {
        reasons.push(PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE);
    }
    let mut system_marked = false;
    let mut session_context_marked = false;
    let mut tail_marked = false;
    let mut tool_schema_marked = false;
    let mut anthropic_system = Vec::new();
    let mut anthropic_messages = Vec::new();
    let mut anthropic_tools = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        if is_system_instruction_role(&message.role) {
            let should_mark =
                Some(index) == system_breakpoint_index && breakpoint_budget.try_take();
            let (block, marked) = text_block(&message.content, should_mark);
            system_marked |= marked;
            anthropic_system.push(block);
            continue;
        }
        let is_session_context_breakpoint = Some(index) == session_context_breakpoint_index;
        let is_tail_breakpoint = tail_breakpoints.contains(&index);
        let is_message_breakpoint = is_session_context_breakpoint || is_tail_breakpoint;
        let should_mark = is_message_breakpoint && breakpoint_budget.try_take();
        let blocks = prompt_cache_tool_wire::anthropic_content_blocks(
            message,
            should_mark,
            &input.model_tools,
        );
        let message_marked = blocks.iter().any(has_cache_control);
        if !message_marked && should_mark {
            breakpoint_budget.release();
            reasons.push(PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE);
        } else if is_message_breakpoint && !should_mark {
            reasons.push(PROMPT_CACHE_WIRE_REASON_BREAKPOINT_BUDGET_EXHAUSTED);
        }
        session_context_marked |= is_session_context_breakpoint && message_marked;
        tail_marked |= is_tail_breakpoint && message_marked;
        push_anthropic_message(
            &mut anthropic_messages,
            anthropic_role(&message.role),
            blocks,
        );
    }

    push_applied(
        &mut applied_hints,
        system_marked,
        PromptCacheHintTarget::LastSystemPrefix,
    );
    push_applied(
        &mut applied_hints,
        session_context_marked,
        PromptCacheHintTarget::LastSessionStableContext,
    );
    push_applied(
        &mut applied_hints,
        tail_marked,
        PromptCacheHintTarget::RecentMessageTail,
    );
    for (index, tool) in input.model_tools.iter().enumerate() {
        let should_mark = tool_schema_should_mark && Some(index) == tool_cache_index;
        tool_schema_marked |= should_mark;
        anthropic_tools.push(anthropic_tool(tool, should_mark));
    }
    if mark_tool_schema && input.model_tools.is_empty() {
        reasons.push(PROMPT_CACHE_WIRE_REASON_TOOL_SCHEMA_EMPTY);
    } else if tool_cache_index.is_some() && !tool_schema_marked {
        reasons.push(PROMPT_CACHE_WIRE_REASON_BREAKPOINT_BUDGET_EXHAUSTED);
    }
    push_applied(
        &mut applied_hints,
        tool_schema_marked,
        PromptCacheHintTarget::LastToolSchema,
    );
    if mark_system && !system_marked {
        reasons.push(PROMPT_CACHE_WIRE_REASON_SYSTEM_EMPTY);
    }

    PromptCacheProviderWirePlan {
        openai_messages: Vec::new(),
        anthropic_system,
        anthropic_messages,
        anthropic_tools,
        reasons: reasons.into_reasons(&applied_hints, input.prompt_cache_plan.provider_policy),
        applied_hints,
        remote_raw_payload_allowed: false,
    }
}

fn push_anthropic_message(
    messages: &mut Vec<serde_json::Value>,
    role: &str,
    blocks: Vec<serde_json::Value>,
) {
    let Some(previous) = messages
        .last_mut()
        .filter(|message| message.get("role").and_then(serde_json::Value::as_str) == Some(role))
    else {
        messages.push(serde_json::json!({
            "role": role,
            "content": blocks,
        }));
        return;
    };
    let Some(content) = previous
        .get_mut("content")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    let first_text = content
        .iter()
        .position(|block| {
            block.get("type").and_then(serde_json::Value::as_str) != Some("tool_result")
        })
        .unwrap_or(content.len());
    let (tool_results, remaining_blocks): (Vec<_>, Vec<_>) =
        blocks.into_iter().partition(|block| {
            block.get("type").and_then(serde_json::Value::as_str) == Some("tool_result")
        });
    content.splice(first_text..first_text, tool_results);
    content.extend(remaining_blocks);
}

fn provider_policy_reasons(input: &PromptCacheProviderWireInput) -> Vec<String> {
    match input.prompt_cache_plan.provider_policy {
        PromptCacheProviderPolicy::OpenAiResponsesPromptCacheKey => {
            vec![PROMPT_CACHE_PROVIDER_REASON_OPENAI_KEY_APPLIED.to_owned()]
        }
        PromptCacheProviderPolicy::AnthropicCacheControl
        | PromptCacheProviderPolicy::StablePrefixOnly => {
            vec![PROMPT_CACHE_PROVIDER_REASON_STABLE_PREFIX_ONLY.to_owned()]
        }
    }
}

fn last_cacheable_system_message_index(
    messages: &[AgentModelMessage],
    mark_system: bool,
) -> Option<usize> {
    if !mark_system {
        return None;
    }
    messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| {
            is_system_instruction_role(&message.role)
                && !message.cache_excluded
                && cacheable_text(&message.content)
        })
        .map(|(index, _)| index)
}

fn last_session_stable_context_message_index(
    messages: &[AgentModelMessage],
    mark_session_context: bool,
) -> Option<usize> {
    if !mark_session_context {
        return None;
    }
    messages
        .iter()
        .enumerate()
        .rev()
        .find(|(_, message)| {
            !is_system_instruction_role(&message.role) && is_session_stable_context_message(message)
        })
        .map(|(index, _)| index)
}

/// 计算滚动双断点应锚定的消息下标集合。
///
/// 取末尾最近 `PROMPT_CACHE_ROLLING_TAIL_BREAKPOINTS` 条非 system 消息；
/// 上一轮末尾断点在本轮自然落入可命中前缀，从而把上轮 tail 转成 READ 命中。
///
/// 跳过 cache_excluded 的临时消息（压缩指令、内部注入等），避免污染缓存断点。
/// 跳过 tool result；工具结果属于本轮动态副作用，不能进入 provider cache breakpoint。
/// 跳过最终 content blocks 无法携带 cache_control 的消息，并继续回补更老候选。
fn rolling_tail_breakpoint_indices(
    messages: &[AgentModelMessage],
    mark_tail: bool,
    model_tools: &[crate::ModelToolDefinition],
) -> (Vec<usize>, bool) {
    if !mark_tail {
        return (Vec::new(), false);
    }
    let mut indices = Vec::new();
    let mut skipped_uncacheable = false;
    for (index, message) in messages.iter().enumerate().rev() {
        if is_system_instruction_role(&message.role)
            || message.role == "tool"
            || message.cache_excluded
            || is_session_stable_context_message(message)
        {
            continue;
        }
        let cacheable =
            prompt_cache_tool_wire::anthropic_content_blocks(message, true, model_tools)
                .iter()
                .any(has_cache_control);
        if !cacheable {
            skipped_uncacheable = true;
            continue;
        }
        indices.push(index);
        if indices.len() == PROMPT_CACHE_ROLLING_TAIL_BREAKPOINTS {
            break;
        }
    }
    (indices, skipped_uncacheable)
}

fn is_system_instruction_role(role: &str) -> bool {
    matches!(role, "system" | "developer")
}

fn is_session_stable_context_message(message: &AgentModelMessage) -> bool {
    cacheable_text(&message.content) && session_stable_context_kind(message).is_some()
}

fn anthropic_role(role: &str) -> &str {
    match role {
        "assistant" => "assistant",
        _ => "user",
    }
}

fn has_hint(hints: &[PromptCacheHint], target: PromptCacheHintTarget) -> bool {
    hints.iter().any(|hint| hint.target == target)
}

fn push_applied(
    applied: &mut Vec<PromptCacheHintTarget>,
    should_push: bool,
    target: PromptCacheHintTarget,
) {
    if should_push {
        applied.push(target);
    }
}

#[derive(Default)]
struct PromptCacheWireReasonCollector {
    skip_reasons: Vec<&'static str>,
}

impl PromptCacheWireReasonCollector {
    fn push(&mut self, reason: &'static str) {
        if !self.skip_reasons.contains(&reason) {
            self.skip_reasons.push(reason);
        }
    }

    fn into_reasons(
        self,
        applied_hints: &[PromptCacheHintTarget],
        provider_policy: PromptCacheProviderPolicy,
    ) -> Vec<String> {
        let mut reasons = Vec::with_capacity(self.skip_reasons.len() + 1);
        if !applied_hints.is_empty() {
            reasons.push(PROMPT_CACHE_WIRE_REASON_HINT_APPLIED.to_owned());
            if provider_policy == PromptCacheProviderPolicy::AnthropicCacheControl {
                reasons.push(PROMPT_CACHE_PROVIDER_REASON_ANTHROPIC_BREAKPOINTS_APPLIED.to_owned());
            }
        }
        reasons.extend(self.skip_reasons.into_iter().map(str::to_owned));
        reasons
    }
}

struct AnthropicBreakpointBudget {
    remaining: usize,
}

impl AnthropicBreakpointBudget {
    const fn new() -> Self {
        Self {
            remaining: ANTHROPIC_CACHE_CONTROL_BREAKPOINT_CAP,
        }
    }

    fn try_take(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }

    fn release(&mut self) {
        self.remaining = (self.remaining + 1).min(ANTHROPIC_CACHE_CONTROL_BREAKPOINT_CAP);
    }
}

fn text_block(content: &str, cache_control: bool) -> (serde_json::Value, bool) {
    if cache_control && cacheable_text(content) {
        return (
            serde_json::json!({
                "type": "text",
                "text": content,
                "cache_control": {"type": PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL},
            }),
            true,
        );
    }
    (
        serde_json::json!({
            "type": "text",
            "text": content,
        }),
        false,
    )
}

fn has_cache_control(block: &serde_json::Value) -> bool {
    block.get("cache_control").is_some()
}

fn cacheable_text(content: &str) -> bool {
    !content.trim().is_empty()
}

pub(crate) fn anthropic_tool(
    tool: &crate::ModelToolDefinition,
    cache_control: bool,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "name": provider_tool_name(&tool.qualified_name),
        "description": tool.description,
        "input_schema": tool.input_schema,
    });
    if cache_control {
        value["cache_control"] = serde_json::json!({
            "type": PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
        });
    }
    value
}
