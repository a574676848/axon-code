use super::*;
use crate::{
    AgentModelMessage, PromptCacheProviderWireFormat, ProviderPromptCachePlanInput,
    ProviderPromptCachePlanRuntime, ProviderToolCall,
};

const TEST_MODEL_ID: &str = "test-model";
const TEST_PROVIDER_NAME: &str = "test-provider";
const TEST_CONTEXT_WINDOW: u64 = 8_192;
const TEST_EDIT_CALL_ID: &str = "call_edit";
const TEST_EDIT_CUSTOM_PROVIDER_NAME: &str = "apply_patch";
const TEST_EDIT_FUNCTION_PROVIDER_NAME: &str = "builtin_edit";
const TEST_EDIT_TOOL_NAME: &str = "builtin.edit";
const TEST_FREEFORM_PATCH: &str =
    "*** Begin Patch\n*** Add File: src/freeform.rs\n+pub fn freeform() {}\n*** End Patch";

#[test]
fn responses_replays_custom_call_before_custom_output() {
    let body = request_body(ProviderStreamProtocol::OpenAiResponses);

    assert_eq!(body["tools"][0]["type"], "custom");
    assert_eq!(body["tools"][0]["name"], TEST_EDIT_CUSTOM_PROVIDER_NAME);
    assert_eq!(body["input"][0]["type"], "custom_tool_call");
    assert_eq!(body["input"][0]["call_id"], TEST_EDIT_CALL_ID);
    assert_eq!(body["input"][0]["name"], TEST_EDIT_CUSTOM_PROVIDER_NAME);
    assert_eq!(body["input"][0]["input"], TEST_FREEFORM_PATCH);
    assert_eq!(body["input"][1]["type"], "custom_tool_call_output");
    assert_eq!(body["input"][1]["call_id"], TEST_EDIT_CALL_ID);
}

#[test]
fn non_responses_providers_use_reversible_function_fallback() {
    for protocol in [
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        let body = request_body(protocol);
        match protocol {
            ProviderStreamProtocol::OpenAiCompatible => {
                assert_eq!(
                    body["tools"][0]["function"]["name"],
                    TEST_EDIT_FUNCTION_PROVIDER_NAME
                );
                let arguments = body["messages"][0]["tool_calls"][0]["function"]["arguments"]
                    .as_str()
                    .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
                    .expect("Chat fallback arguments 应为合法 JSON");
                assert_eq!(arguments["input"], TEST_FREEFORM_PATCH);
            }
            ProviderStreamProtocol::AnthropicMessages => {
                assert_eq!(body["tools"][0]["name"], TEST_EDIT_FUNCTION_PROVIDER_NAME);
                assert_eq!(
                    body["messages"][0]["content"][0]["input"]["input"],
                    TEST_FREEFORM_PATCH
                );
            }
            ProviderStreamProtocol::OpenAiResponses => unreachable!(),
        }
    }
}

fn request_body(protocol: ProviderStreamProtocol) -> serde_json::Value {
    let model_tools = vec![edit_tool()];
    let messages = freeform_patch_messages();
    let prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format: match protocol {
                ProviderStreamProtocol::OpenAiResponses => {
                    PromptCacheProviderWireFormat::OpenAiResponses
                }
                ProviderStreamProtocol::OpenAiCompatible => {
                    PromptCacheProviderWireFormat::OpenAiCompatibleChat
                }
                ProviderStreamProtocol::AnthropicMessages => {
                    PromptCacheProviderWireFormat::AnthropicMessages
                }
            },
            model_id: TEST_MODEL_ID.to_owned(),
            context_window: TEST_CONTEXT_WINDOW,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone().into(),
            model_tools: model_tools.clone(),
            context_section_count: 1,
        })
        .expect("freeform 请求应生成 prompt cache plan");
    let plan = ProviderStreamingHttpRequestRuntime
        .plan(ProviderStreamingHttpRequestInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            protocol,
            base_url: "https://provider.example/v1".to_owned(),
            model_id: TEST_MODEL_ID.to_owned(),
            messages: messages.into(),
            attachment_archive_root: None,
            model_tools,
            prompt_cache_plan,
            api_key_env: None,
            api_key: None,
            service_tier: None,
            reasoning_effort: None,
            reasoning_summary: None,
            structured_output: None,
            anthropic_max_tokens: TEST_CONTEXT_WINDOW,
            capabilities: axon_config::ProviderCapabilities::stateless(),
            supports_vision: false,
            server_conversation_state_ref: None,
        })
        .expect("freeform 请求应映射到 Provider 原生协议");
    serde_json::from_str(&plan.body_text).expect("请求体应为合法 JSON")
}

fn edit_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: TEST_EDIT_TOOL_NAME.to_owned(),
        description: "编辑文件。".to_owned(),
        schema_hash: "tool-schema-edit-freeform".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "input": { "type": "string" } },
            "required": ["input"],
            "additionalProperties": false
        }),
        input_format: axon_domain::ToolInputFormat::Freeform {
            fallback_field: "input".to_owned(),
            format: axon_domain::ToolFreeformFormat {
                format_type: "grammar".to_owned(),
                syntax: "lark".to_owned(),
                definition: "start: PATCH\nPATCH: /[\\s\\S]+/".to_owned(),
            },
        },
    }
}

fn freeform_patch_messages() -> Vec<AgentModelMessage> {
    crate::agent_run_tool_result_messages(
        &[ProviderToolCall::new(
            TEST_EDIT_CALL_ID,
            TEST_EDIT_TOOL_NAME,
            axon_domain::ToolCallInput::text(TEST_FREEFORM_PATCH),
        )],
        &[crate::AgentRunToolResultReceipt {
            provider_call_id: TEST_EDIT_CALL_ID.to_owned(),
            tool_name: TEST_EDIT_TOOL_NAME.to_owned(),
            status: "completed".to_owned(),
            summary: "patch applied".to_owned(),
            metadata: None,
            output_ref: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: true,
        }],
        None,
        crate::AgentRunToolOutputPolicy::default(),
    )
    .expect("freeform 工具调用与结果应生成历史")
}
