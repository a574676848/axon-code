//! Soul prompt cache 边界集成验证。

use axon_agent_runtime::{
    AgentModelMessage, PromptCacheProviderWireFormat, PromptCacheSegmentKind,
    ProviderPromptCachePlan, ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
};

const TEST_CONTEXT_WINDOW: u64 = 8_192;
const TEST_SOUL_START: &str = "<assistant-profile-context>";
const TEST_SOUL_END: &str = "</assistant-profile-context>";

#[test]
fn soul_change_only_invalidates_soul_stable_segment() {
    let first = prompt_cache_plan("soul-v1");
    let second = prompt_cache_plan("soul-v2");

    assert_eq!(first.namespace_key, second.namespace_key);
    assert_eq!(first.stable_prefix_hash, second.stable_prefix_hash);
    assert_ne!(
        first.session_stable_context_hash,
        second.session_stable_context_hash
    );
    assert!(
        first
            .segments
            .iter()
            .any(|segment| segment.kind == PromptCacheSegmentKind::Soul)
    );
}

fn prompt_cache_plan(soul: &str) -> ProviderPromptCachePlan {
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_OPENAI,
            ),
            format: PromptCacheProviderWireFormat::OpenAiResponses,
            model_id: "test-model".to_owned(),
            context_window: TEST_CONTEXT_WINDOW,
            supports_tools: true,
            supports_vision: false,
            messages: vec![
                AgentModelMessage::context("system", "stable-system"),
                AgentModelMessage::context(
                    "system",
                    format!("{TEST_SOUL_START}\n{soul}\n{TEST_SOUL_END}"),
                ),
            ]
            .into(),
            model_tools: Vec::new(),
            context_section_count: 1,
        })
        .unwrap()
}
