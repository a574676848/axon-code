use super::{
    PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL, PROMPT_CACHE_WIRE_REASON_HINT_APPLIED,
    PROMPT_CACHE_WIRE_REASON_SYSTEM_EMPTY, PromptCacheHintTarget, PromptCacheProviderWireFormat,
    PromptCacheProviderWireInput, PromptCacheProviderWirePlan, PromptCacheProviderWireRuntime,
};

use crate::{
    AgentModelMessage, ProviderPromptCachePlan, ProviderPromptCachePlanInput,
    ProviderPromptCachePlanRuntime,
};

#[test]
fn prompt_cache_wire_marks_only_last_cacheable_system_prompt() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "stable system"),
            AgentModelMessage::text("system", "durable context"),
            AgentModelMessage::text("user", "latest user"),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(plan.anthropic_system.len(), 2);
    assert!(plan.anthropic_system[0].get("cache_control").is_none());
    assert_eq!(
        plan.anthropic_system[1]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(anthropic_cache_control_count(&plan), 2);
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::LastSystemPrefix)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_HINT_APPLIED
    ));
}

#[test]
fn prompt_cache_wire_empty_system_prompts_do_not_consume_breakpoint_budget() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "   "),
            AgentModelMessage::text("system", ""),
            AgentModelMessage::text("user", "first"),
            AgentModelMessage::text("assistant", "second"),
            AgentModelMessage::text("user", "third"),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert!(plan.anthropic_system[0].get("cache_control").is_none());
    assert!(plan.anthropic_system[1].get("cache_control").is_none());
    assert_eq!(anthropic_cache_control_count(&plan), 2);
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::LastSystemPrefix)
    );
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_SYSTEM_EMPTY
    ));
}

fn anthropic_cache_control_count(plan: &PromptCacheProviderWirePlan) -> usize {
    let system = plan
        .anthropic_system
        .iter()
        .filter(|block| block.get("cache_control").is_some())
        .count();
    let messages = plan
        .anthropic_messages
        .iter()
        .filter_map(|message| message.get("content").and_then(serde_json::Value::as_array))
        .flatten()
        .filter(|block| block.get("cache_control").is_some())
        .count();
    let tools = plan
        .anthropic_tools
        .iter()
        .filter(|tool| tool.get("cache_control").is_some())
        .count();
    system + messages + tools
}

fn has_reason(reasons: &[String], reason: &str) -> bool {
    reasons.iter().any(|item| item == reason)
}

fn test_prompt_cache_plan(format: PromptCacheProviderWireFormat) -> ProviderPromptCachePlan {
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
            ),
            format,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: Vec::new().into(),
            model_tools: Vec::new(),
            context_section_count: 0,
        })
        .unwrap()
}
