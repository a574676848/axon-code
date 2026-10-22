use super::*;
use crate::{AgentModelMessage, ProviderStructuredOutputContract, ProviderStructuredOutputMode};

const TEST_PROVIDER_NAME: &str = "structured-output-test";
const TEST_MODEL_ID: &str = "structured-output-model";
const TEST_CONTEXT_WINDOW: u64 = 8_192;

#[test]
fn structured_output_uses_native_protocol_fields_and_preserves_prompt_cache_plan() {
    let schema = test_schema();

    for protocol in protocols() {
        let ordinary = request_input(protocol, None);
        let structured = request_input(protocol, Some(json_schema_contract(schema.clone())));

        assert_eq!(
            ordinary.prompt_cache_plan.namespace_key,
            structured.prompt_cache_plan.namespace_key
        );
        assert_eq!(
            ordinary.prompt_cache_plan.stable_prefix_hash,
            structured.prompt_cache_plan.stable_prefix_hash
        );
        assert_eq!(
            ordinary.prompt_cache_plan.cache_hints(),
            structured.prompt_cache_plan.cache_hints()
        );

        let plan = ProviderStreamingHttpRequestRuntime
            .plan(structured)
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
        let envelope = match protocol {
            ProviderStreamProtocol::OpenAiResponses => &body["text"]["format"],
            ProviderStreamProtocol::OpenAiCompatible => &body["response_format"],
            ProviderStreamProtocol::AnthropicMessages => &body["output_config"]["format"],
        };
        let format = if protocol == ProviderStreamProtocol::OpenAiCompatible {
            &envelope["json_schema"]
        } else {
            envelope
        };

        assert_eq!(envelope["type"], "json_schema");
        assert_eq!(format["schema"], schema);
        if protocol != ProviderStreamProtocol::AnthropicMessages {
            assert_eq!(format["name"], "axon_output");
            assert_eq!(format["strict"], true);
        }
        if protocol == ProviderStreamProtocol::OpenAiResponses {
            assert_eq!(
                body["prompt_cache_key"],
                ordinary.prompt_cache_plan.namespace_key
            );
        }
        if protocol == ProviderStreamProtocol::AnthropicMessages {
            assert_eq!(body["output_config"]["effort"], "high");
        }
    }
}

#[test]
fn ordinary_requests_do_not_emit_structured_output_fields() {
    for protocol in protocols() {
        let plan = ProviderStreamingHttpRequestRuntime
            .plan(request_input(protocol, None))
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

        assert!(body.pointer("/text/format").is_none());
        assert!(body.get("response_format").is_none());
        assert!(body.pointer("/output_config/format").is_none());
    }
}

#[test]
fn prompt_only_contract_moves_schema_into_system_prompt_without_native_fields() {
    let schema = test_schema();

    for protocol in protocols() {
        let structured = request_input(protocol, Some(prompt_only_contract(schema.clone())));
        let plan = ProviderStreamingHttpRequestRuntime
            .plan(structured)
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

        assert!(body.pointer("/text/format").is_none());
        assert!(body.get("response_format").is_none());
        assert!(body.pointer("/output_config/format").is_none());

        let prompt_text = match protocol {
            ProviderStreamProtocol::OpenAiResponses => {
                body["instructions"].as_str().map(str::to_owned)
            }
            ProviderStreamProtocol::OpenAiCompatible => {
                body["messages"][0]["content"].as_str().map(str::to_owned)
            }
            ProviderStreamProtocol::AnthropicMessages => {
                serde_json::to_string(&body["system"]).ok()
            }
        }
        .expect("prompt-only 降级后 system prompt 必须存在");
        assert!(prompt_text.contains("JSON Schema"));
        assert!(prompt_text.contains("answer"));
    }
}

fn json_schema_contract(schema: serde_json::Value) -> ProviderStructuredOutputContract {
    ProviderStructuredOutputContract {
        schema,
        mode: ProviderStructuredOutputMode::JsonSchema,
    }
}

fn prompt_only_contract(schema: serde_json::Value) -> ProviderStructuredOutputContract {
    ProviderStructuredOutputContract {
        schema,
        mode: ProviderStructuredOutputMode::PromptOnly,
    }
}

fn protocols() -> [ProviderStreamProtocol; 3] {
    [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ]
}

fn request_input(
    protocol: ProviderStreamProtocol,
    structured_output: Option<ProviderStructuredOutputContract>,
) -> ProviderStreamingHttpRequestInput {
    let capabilities = match protocol {
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
    let messages = vec![
        AgentModelMessage::text("system", "stable system"),
        AgentModelMessage::text("user", "dynamic task"),
    ];
    let prompt_cache_plan = crate::ProviderPromptCachePlanRuntime
        .plan(crate::ProviderPromptCachePlanInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities,
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
            model_id: TEST_MODEL_ID.to_owned(),
            context_window: TEST_CONTEXT_WINDOW,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone().into(),
            model_tools: Vec::new(),
            context_section_count: 1,
        })
        .unwrap();
    ProviderStreamingHttpRequestInput {
        provider_name: TEST_PROVIDER_NAME.to_owned(),
        protocol,
        base_url: "https://provider.example/v1".to_owned(),
        model_id: TEST_MODEL_ID.to_owned(),
        messages: messages.into(),
        attachment_archive_root: None,
        model_tools: Vec::new(),
        prompt_cache_plan,
        api_key_env: None,
        api_key: None,
        service_tier: None,
        reasoning_effort: (protocol == ProviderStreamProtocol::AnthropicMessages)
            .then(|| "high".to_owned()),
        reasoning_summary: Some("auto".to_owned()),
        structured_output,
        anthropic_max_tokens: TEST_CONTEXT_WINDOW,
        capabilities,
        supports_vision: false,
        server_conversation_state_ref: None,
    }
}

fn test_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "answer": {"type": "string"}
        },
        "required": ["answer"],
        "additionalProperties": false
    })
}
