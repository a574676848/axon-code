use super::{
    PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE, PromptCacheHintTarget,
    PromptCacheProviderWireFormat, PromptCacheProviderWireInput, PromptCacheProviderWireRuntime,
};

use crate::{
    AgentModelMessage, AgentModelToolCallMetadata, AnthropicThinkingBlockMetadata,
    ProviderPromptCachePlan, ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
};

#[test]
fn prompt_cache_wire_projects_anthropic_thinking_without_cache_control() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("assistant", "内部思考不应作为 text 回放")
                .with_anthropic_thinking_blocks(vec![
                    AnthropicThinkingBlockMetadata {
                        block_type: "thinking".to_owned(),
                        thinking: Some("long deliberation".to_owned()),
                        data: None,
                        signature: Some("sig-thinking".to_owned()),
                    },
                    AnthropicThinkingBlockMetadata {
                        block_type: "redacted_thinking".to_owned(),
                        thinking: None,
                        data: Some("encrypted-thinking".to_owned()),
                        signature: None,
                    },
                ]),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    let blocks = plan.anthropic_messages[0]["content"]
        .as_array()
        .expect("Anthropic content 应是 block 数组");
    assert_eq!(blocks[0]["type"], "thinking");
    assert_eq!(blocks[0]["thinking"], "long deliberation");
    assert_eq!(blocks[0]["signature"], "sig-thinking");
    assert_eq!(blocks[1]["type"], "redacted_thinking");
    assert_eq!(blocks[1]["data"], "encrypted-thinking");
    assert!(
        blocks
            .iter()
            .all(|block| block.get("cache_control").is_none())
    );
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE
    ));
}

#[test]
fn prompt_cache_wire_rejects_unsigned_anthropic_thinking_block() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        ),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("assistant", "fallback text must not be cached")
                .with_anthropic_thinking_blocks(vec![AnthropicThinkingBlockMetadata {
                    block_type: "thinking".to_owned(),
                    thinking: Some("private reasoning".to_owned()),
                    data: None,
                    signature: None,
                }]),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(PromptCacheProviderWireFormat::AnthropicMessages),
    });

    let blocks = plan.anthropic_messages[0]["content"]
        .as_array()
        .expect("Anthropic content 应是 block 数组");
    assert!(blocks.is_empty());
    assert!(
        !plan
            .applied_hints
            .contains(&PromptCacheHintTarget::RecentMessageTail)
    );
    assert!(has_reason(
        &plan.reasons,
        PROMPT_CACHE_WIRE_REASON_TAIL_UNCACHEABLE
    ));
}

#[test]
fn prompt_cache_message_metadata_builders_preserve_existing_fields() {
    let message = AgentModelMessage::text("assistant", "")
        .with_tool_calls(vec![AgentModelToolCallMetadata {
            id: "call_search".to_owned(),
            name: "builtin_search".to_owned(),
            input: "{}".into(),
        }])
        .with_openai_reasoning_content("openai reasoning");

    let metadata = message.provider_metadata.expect("provider metadata");
    assert_eq!(
        metadata.openai_reasoning_content.as_deref(),
        Some("openai reasoning")
    );
    assert_eq!(metadata.tool_calls.as_ref().map(Vec::len), Some(1));
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
