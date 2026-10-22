use super::*;
use crate::AgentModelMessage;

pub fn request_input(model_tools: Vec<ModelToolDefinition>) -> ProviderStreamingHttpRequestInput {
    let messages = vec![AgentModelMessage {
        role: "user".to_owned(),
        content: "ping".to_owned(),
        provider_metadata: None,
        local_images: Vec::new(),
        local_files: Vec::new(),
        cache_excluded: false,
        source: crate::AgentModelMessageSource::Conversation,
        kind: crate::AgentModelMessageKind::Regular,
        retention_class: crate::RetentionClass::P0Contract,
        source_ref: None,
    }];
    let prompt_cache_plan = crate::ProviderPromptCachePlanRuntime
        .plan(crate::ProviderPromptCachePlanInput {
            provider_name: "openai-compatible".to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format: crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone().into(),
            model_tools: model_tools.clone(),
            context_section_count: 1,
        })
        .unwrap();
    ProviderStreamingHttpRequestInput {
        provider_name: "openai-compatible".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        supports_vision: false,
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        base_url: "https://provider.example/v1".to_owned(),
        model_id: "test-model".to_owned(),
        messages: messages.into(),
        attachment_archive_root: None,
        model_tools,
        prompt_cache_plan,
        api_key_env: None,
        api_key: None,
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
        structured_output: None,
        anthropic_max_tokens: 8_192,
        server_conversation_state_ref: None,
    }
}

pub fn request_prompt_cache_plan(
    input: &ProviderStreamingHttpRequestInput,
) -> crate::ProviderPromptCachePlan {
    crate::ProviderPromptCachePlanRuntime
        .plan(crate::ProviderPromptCachePlanInput {
            provider_name: input.provider_name.clone(),
            capabilities: input.capabilities,
            format: match input.protocol {
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
            model_id: input.model_id.clone(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: input.supports_vision,
            messages: input.messages.clone(),
            model_tools: input.model_tools.clone(),
            context_section_count: 1,
        })
        .unwrap()
}

pub fn refresh_prompt_cache_plan(input: &mut ProviderStreamingHttpRequestInput) {
    input.capabilities = match input.protocol {
        ProviderStreamProtocol::OpenAiResponses => {
            axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI)
        }
        ProviderStreamProtocol::AnthropicMessages => {
            axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
            )
        }
        ProviderStreamProtocol::OpenAiCompatible => axon_config::ProviderCapabilities::stateless(),
    };
    input.prompt_cache_plan = request_prompt_cache_plan(input);
}

pub fn model_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "builtin.shell_command".to_owned(),
        description: "执行本机 shell 命令，需要用户审批后执行。".to_owned(),
        schema_hash: "tool-schema-shell-command".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "command": { "type": "string" } },
            "required": ["command"]
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

pub fn search_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "builtin.search".to_owned(),
        description: "只读搜索当前工作区。".to_owned(),
        schema_hash: "tool-schema-search".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}
