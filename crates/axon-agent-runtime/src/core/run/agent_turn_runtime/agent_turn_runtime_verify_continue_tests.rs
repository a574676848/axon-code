use super::*;
use crate::{
    AgentModelMessage, ModelToolDefinition, PromptCacheProviderWireFormat,
    ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime, ProviderStreamProtocol,
    ProviderStreamingTransportInput, ProviderStreamingWireTransport,
};
use axon_client_identity::ClientChannel;

#[test]
fn tool_receipts_keep_tools_available_until_protocol_stop() {
    let input = ProviderStreamingTransportInput {
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
        structured_output: None,
        context_window: 32_000,
        supports_tools: true,
        supports_vision: false,
        messages: vec![AgentModelMessage::text("user", "ping")].into(),
        model_tools: model_tools(),
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan: prompt_cache_plan(),
        anthropic_max_tokens: 4096,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
    };
    let provider_calls = std::cell::Cell::new(0);
    let tool_turns = std::cell::Cell::new(0);
    let report = AgentTurnRuntime
        .run(
            &input,
            |request| {
                let call = provider_calls.get();
                provider_calls.set(call + 1);
                let body: serde_json::Value =
                    serde_json::from_str(&request.request.body_text).unwrap();
                assert_eq!(body["tools"].as_array().map(Vec::len), Some(2));
                crate::test_stream_outcome(provider_body(call))
            },
            |_report, _provider_history, _turn_id| {
                let tool_turn = tool_turns.get();
                tool_turns.set(tool_turn + 1);
                let (call_id, receipt) = if tool_turn == 0 {
                    ("call_write", "verify=passed")
                } else {
                    ("call_search", "search=completed")
                };
                Ok::<_, axon_domain::AxonError>(AgentTurnToolDecision::Continue {
                    receipts: vec![receipt.to_owned()],
                    next_messages: vec![AgentModelMessage::tool_result(
                        call_id,
                        format!("status=completed {receipt}"),
                        true,
                    )],
                })
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(provider_calls.get(), 3);
    assert_eq!(tool_turns.get(), 2);
    assert_eq!(report.content, "工具工作完成");
    assert_eq!(
        report.tool_receipts,
        vec!["verify=passed", "search=completed"]
    );
}

fn provider_body(call: u32) -> &'static str {
    match call {
        0 => {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_write\",\"type\":\"function\",\"function\":{\"name\":\"builtin_write\",\"arguments\":\"{\\\"path\\\":\\\"src/lib.rs\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n"
        }
        1 => {
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_search\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"verify\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n"
        }
        _ => {
            "data: {\"choices\":[{\"delta\":{\"content\":\"工具工作完成\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n"
        }
    }
}

fn model_tools() -> Vec<ModelToolDefinition> {
    vec![model_tool("builtin.write"), model_tool("builtin.search")]
}

fn model_tool(name: &str) -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: name.to_owned(),
        description: "测试工具".to_owned(),
        schema_hash: format!("tool-schema-{name}"),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "query": { "type": "string" } },
            "required": ["query"]
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

fn prompt_cache_plan() -> crate::ProviderPromptCachePlan {
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
            model_id: "test-model".to_owned(),
            context_window: 32_000,
            supports_tools: true,
            supports_vision: false,
            messages: vec![AgentModelMessage::text("user", "ping")].into(),
            model_tools: model_tools(),
            context_section_count: 1,
        })
        .unwrap()
}
