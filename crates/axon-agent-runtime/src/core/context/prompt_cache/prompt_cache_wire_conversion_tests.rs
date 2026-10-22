use super::{
    PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL, PromptCacheHintTarget, PromptCacheProviderWireFormat,
    PromptCacheProviderWireInput, PromptCacheProviderWireRuntime,
};

use crate::{
    AgentModelMessage, ProviderPromptCachePlan, ProviderPromptCachePlanInput,
    ProviderPromptCachePlanRuntime,
};

#[test]
fn prompt_cache_wire_keeps_cache_control_on_assistant_text_with_tool_use() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "系统指令"),
            AgentModelMessage::text("assistant", "需要先查询资料").with_tool_calls(vec![
                crate::AgentModelToolCallMetadata {
                    id: "call_search".to_owned(),
                    name: "builtin_search".to_owned(),
                    input: r#"{"query":"provider"}"#.into(),
                },
            ]),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(
        plan.anthropic_system[0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(plan.anthropic_messages[0]["content"][0]["type"], "text");
    assert_eq!(
        plan.anthropic_messages[0]["content"][0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(plan.anthropic_messages[0]["content"][1]["type"], "tool_use");
    assert!(
        plan.applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
}

#[test]
fn prompt_cache_wire_maps_developer_messages_to_anthropic_system() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("developer", "开发者约束"),
            AgentModelMessage::text("user", "开始"),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    assert_eq!(plan.anthropic_system[0]["text"], "开发者约束");
    assert_eq!(plan.anthropic_messages.len(), 1);
    assert_eq!(plan.anthropic_messages[0]["role"], "user");
    assert_eq!(
        plan.anthropic_system[0]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
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
