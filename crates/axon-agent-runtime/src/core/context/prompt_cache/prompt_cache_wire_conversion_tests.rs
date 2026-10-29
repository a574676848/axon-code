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

fn chat_capabilities(requires_reasoning_content: bool) -> axon_config::ProviderCapabilities {
    let mut capabilities =
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI_CHAT);
    capabilities.requires_reasoning_content_on_assistant_messages = requires_reasoning_content;
    capabilities
}

fn chat_wire_input(
    format: PromptCacheProviderWireFormat,
    requires_reasoning_content: bool,
) -> PromptCacheProviderWireInput {
    PromptCacheProviderWireInput {
        format,
        capabilities: chat_capabilities(requires_reasoning_content),
        supports_vision: false,
        messages: vec![
            AgentModelMessage::text("system", "系统指令"),
            AgentModelMessage::text("user", "开始"),
            AgentModelMessage::text("assistant", "先读 README")
                .with_openai_reasoning_content("需要先确认现状"),
            AgentModelMessage::text("assistant", "").with_tool_calls(vec![
                crate::AgentModelToolCallMetadata {
                    id: "call_1".to_owned(),
                    name: "builtin_read".to_owned(),
                    input: r#"{"path":"README.md"}"#.into(),
                },
            ]),
            AgentModelMessage::tool_result("call_1", "README 内容", true),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(format),
    }
}

#[test]
fn prompt_cache_wire_backfills_reasoning_content_for_chat_assistant_messages() {
    let plan = PromptCacheProviderWireRuntime.lower(&chat_wire_input(
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        true,
    ));

    // 已有推理正文保持不变；缺失的历史 assistant 消息补空串。
    assert_eq!(
        plan.openai_messages[2]["reasoning_content"],
        "需要先确认现状"
    );
    assert_eq!(plan.openai_messages[3]["reasoning_content"], "");
    // user 与 tool 消息不得携带推理正文。
    assert!(plan.openai_messages[1].get("reasoning_content").is_none());
    assert!(plan.openai_messages[4].get("reasoning_content").is_none());
}

#[test]
fn prompt_cache_wire_skips_reasoning_content_backfill_when_compat_is_off() {
    let plan = PromptCacheProviderWireRuntime.lower(&chat_wire_input(
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        false,
    ));

    assert_eq!(
        plan.openai_messages[2]["reasoning_content"],
        "需要先确认现状"
    );
    assert!(plan.openai_messages[3].get("reasoning_content").is_none());
}

#[test]
fn prompt_cache_wire_does_not_backfill_responses_messages() {
    let plan = PromptCacheProviderWireRuntime.lower(&chat_wire_input(
        PromptCacheProviderWireFormat::OpenAiResponses,
        true,
    ));

    assert!(plan.openai_messages[3].get("reasoning_content").is_none());
}
