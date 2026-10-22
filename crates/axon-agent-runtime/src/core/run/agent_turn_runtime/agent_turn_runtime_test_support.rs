use crate::{
    AgentModelMessage, ModelToolDefinition, ProviderStreamProtocol,
    ProviderStreamingTransportInput, ProviderStreamingWireTransport,
};
use axon_client_identity::ClientChannel;

pub(super) fn test_transport_input() -> ProviderStreamingTransportInput {
    ProviderStreamingTransportInput {
        client_channel: ClientChannel::Cli,
        turn_id: axon_domain::TurnId(1),
        provider_name: "test-provider".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        base_url: "https://provider.example/v1".to_owned(),
        api_key_env: None,
        api_key: None,
        model_id: "test-model".to_owned(),
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
        context_window: 32_000,
        supports_tools: true,
        supports_vision: false,
        messages: vec![AgentModelMessage::text("user", "ping")].into(),
        model_tools: Vec::new(),
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan: transport_prompt_cache_plan(),
        anthropic_max_tokens: 4096,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
        structured_output: None,
    }
}

pub(super) fn transport_prompt_cache_plan() -> crate::ProviderPromptCachePlan {
    transport_prompt_cache_plan_for(ProviderStreamProtocol::OpenAiCompatible, Vec::new())
}

pub(super) fn transport_prompt_cache_plan_for(
    protocol: ProviderStreamProtocol,
    model_tools: Vec<ModelToolDefinition>,
) -> crate::ProviderPromptCachePlan {
    crate::ProviderPromptCachePlanRuntime
        .plan(crate::ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format: match protocol {
                ProviderStreamProtocol::OpenAiResponses => {
                    crate::PromptCacheProviderWireFormat::OpenAiResponses
                }
                ProviderStreamProtocol::OpenAiCompatible => {
                    crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat
                }
                ProviderStreamProtocol::AnthropicMessages => {
                    crate::PromptCacheProviderWireFormat::AnthropicMessages
                }
            },
            model_id: "test-model".to_owned(),
            context_window: 32_000,
            supports_tools: true,
            supports_vision: false,
            messages: vec![AgentModelMessage::text("user", "ping")].into(),
            model_tools,
            context_section_count: 1,
        })
        .unwrap()
}

pub(super) fn deferred_model_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "mcp__team__query_db".to_owned(),
        description: "Query team database".to_owned(),
        schema_hash: "tool-schema-query-db".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}
