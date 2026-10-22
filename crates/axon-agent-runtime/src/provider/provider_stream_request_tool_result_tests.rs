use super::{
    ProviderStreamProtocol, ProviderStreamingHttpRequestInput, ProviderStreamingHttpRequestRuntime,
};
use crate::{
    AgentRunToolOutputPolicy, AgentRunToolResultReceipt, ModelToolDefinition,
    PromptCacheProviderWireFormat, ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
    ProviderToolCall, agent_run_tool_result_messages,
};

const TEST_MODEL_ID: &str = "test-model";
const TEST_PROVIDER_NAME: &str = "test-provider";
const TEST_CONTEXT_WINDOW: u64 = 8_192;
const TEST_CALL_ID: &str = "call-list";
const TEST_TOOL_NAME: &str = "builtin.glob";
const TEST_PROVIDER_TOOL_NAME: &str = "builtin_glob";
const TEST_ARGUMENTS: &str = r#"{"paths":["."]}"#;
const TEST_OUTPUT: &str = "glob completed";
const OVERSIZED_ENTRY_COUNT: usize = 300;

#[test]
fn structured_tool_history_maps_to_each_provider_native_protocol() {
    for protocol in protocols() {
        let body = request_body(protocol, completed_messages(TEST_OUTPUT));
        match protocol {
            ProviderStreamProtocol::OpenAiResponses => assert_responses_wire(&body),
            ProviderStreamProtocol::OpenAiCompatible => assert_chat_wire(&body),
            ProviderStreamProtocol::AnthropicMessages => assert_anthropic_wire(&body, false),
        }
    }
}

#[test]
fn failed_tool_feedback_stays_inside_the_same_result() {
    for protocol in protocols() {
        let body = request_body(protocol, failed_messages());
        let output = tool_output(&body, protocol);
        let payload: serde_json::Value =
            serde_json::from_str(output).expect("失败工具结果必须保持合法 JSON");

        assert_eq!(payload["status"], "failed");
        assert!(payload["feedback"].get("corrected_tool").is_none());
        assert_eq!(payload["feedback"]["missing_fields"][0], "path");
        assert!(payload["feedback"]["retry_hint"].is_string());
        assert!(
            !body
                .to_string()
                .contains("<runtime-note kind=\"tool-feedback\">")
        );
        if protocol == ProviderStreamProtocol::AnthropicMessages {
            assert_anthropic_wire(&body, true);
        }
    }
}

#[test]
fn oversized_tool_output_preserves_completion_envelope_for_all_protocols() {
    for protocol in protocols() {
        let body = request_body(protocol, oversized_messages());
        let output = tool_output(&body, protocol);
        let payload: serde_json::Value =
            serde_json::from_str(output).expect("截断后的工具结果必须保持合法 JSON");

        assert_eq!(payload["tool"], TEST_TOOL_NAME);
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["summary"], TEST_OUTPUT);
        assert_eq!(payload["metadata"]["truncated"], true);
        assert_eq!(payload["metadata"]["marker"], "tool output truncated");
    }
}

fn protocols() -> [ProviderStreamProtocol; 3] {
    [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ]
}

fn request_body(
    protocol: ProviderStreamProtocol,
    messages: Vec<crate::AgentModelMessage>,
) -> serde_json::Value {
    let model_tools = vec![model_tool()];
    let format = match protocol {
        ProviderStreamProtocol::OpenAiResponses => PromptCacheProviderWireFormat::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible => {
            PromptCacheProviderWireFormat::OpenAiCompatibleChat
        }
        ProviderStreamProtocol::AnthropicMessages => {
            PromptCacheProviderWireFormat::AnthropicMessages
        }
    };
    let prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: TEST_PROVIDER_NAME.to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format,
            model_id: TEST_MODEL_ID.to_owned(),
            context_window: TEST_CONTEXT_WINDOW,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone().into(),
            model_tools: model_tools.clone(),
            context_section_count: 1,
        })
        .expect("prompt cache plan 应生成");
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
            anthropic_max_tokens: 8_192,
            capabilities: axon_config::ProviderCapabilities::stateless(),
            supports_vision: false,
            server_conversation_state_ref: None,
        })
        .expect("三种 Provider 协议都应生成请求计划");
    serde_json::from_str(&plan.body_text).expect("请求体应为合法 JSON")
}

fn completed_messages(output: &str) -> Vec<crate::AgentModelMessage> {
    messages(
        receipt("completed", output, None),
        ProviderToolCall::new(TEST_CALL_ID, TEST_TOOL_NAME, TEST_ARGUMENTS),
    )
}

fn failed_messages() -> Vec<crate::AgentModelMessage> {
    messages(
        receipt(
            "failed",
            "builtin.glob paths 必须是字符串数组",
            Some("execution_failed"),
        ),
        ProviderToolCall::new(TEST_CALL_ID, TEST_TOOL_NAME, "{}"),
    )
}

fn oversized_messages() -> Vec<crate::AgentModelMessage> {
    let mut result = receipt("completed", TEST_OUTPUT, None);
    result.metadata = Some(serde_json::json!({
        "entries": (0..OVERSIZED_ENTRY_COUNT)
            .map(|index| serde_json::json!({
                "name": format!("entry-{index:03}"),
                "isDir": true,
            }))
            .collect::<Vec<_>>(),
    }));
    messages_with_policy(
        result,
        ProviderToolCall::new(TEST_CALL_ID, TEST_TOOL_NAME, TEST_ARGUMENTS),
        AgentRunToolOutputPolicy::with_max_output_bytes(4_000),
    )
}

fn messages(
    result: AgentRunToolResultReceipt,
    call: ProviderToolCall,
) -> Vec<crate::AgentModelMessage> {
    messages_with_policy(result, call, AgentRunToolOutputPolicy::default())
}

fn messages_with_policy(
    result: AgentRunToolResultReceipt,
    call: ProviderToolCall,
    output_policy: AgentRunToolOutputPolicy,
) -> Vec<crate::AgentModelMessage> {
    agent_run_tool_result_messages(&[call], &[result], None, output_policy)
        .expect("工具调用与结果应生成强类型历史")
}

fn receipt(status: &str, summary: &str, error_type: Option<&str>) -> AgentRunToolResultReceipt {
    AgentRunToolResultReceipt {
        provider_call_id: TEST_CALL_ID.to_owned(),
        tool_name: TEST_TOOL_NAME.to_owned(),
        status: status.to_owned(),
        summary: summary.to_owned(),
        metadata: (status == "failed").then(|| serde_json::json!({"missingFields": ["path"]})),
        output_ref: None,
        error_type: error_type.map(ToOwned::to_owned),
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
    }
}

fn assert_responses_wire(body: &serde_json::Value) {
    let input = body["input"].as_array().expect("Responses input 应为数组");
    let call = input
        .iter()
        .find(|item| item["type"] == "function_call")
        .expect("Responses 应包含 function_call");
    let result = input
        .iter()
        .find(|item| item["type"] == "function_call_output")
        .expect("Responses 应包含 function_call_output");

    assert_eq!(call["call_id"], TEST_CALL_ID);
    assert_eq!(call["name"], TEST_PROVIDER_TOOL_NAME);
    assert_eq!(call["arguments"], TEST_ARGUMENTS);
    assert_eq!(call["status"], "completed");
    assert_eq!(result["call_id"], TEST_CALL_ID);
}

fn assert_chat_wire(body: &serde_json::Value) {
    let messages = body["messages"].as_array().expect("Chat messages 应为数组");
    let call = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("Chat 应包含 assistant tool_calls");
    let result = messages
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("Chat 应包含 role=tool");

    assert_eq!(call["tool_calls"][0]["id"], TEST_CALL_ID);
    assert_eq!(
        call["tool_calls"][0]["function"]["name"],
        TEST_PROVIDER_TOOL_NAME
    );
    assert_eq!(
        call["tool_calls"][0]["function"]["arguments"],
        TEST_ARGUMENTS
    );
    assert_eq!(result["tool_call_id"], TEST_CALL_ID);
}

fn assert_anthropic_wire(body: &serde_json::Value, is_error: bool) {
    let messages = body["messages"]
        .as_array()
        .expect("Anthropic messages 应为数组");
    let call = messages[0]["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|block| block["type"] == "tool_use"))
        .expect("Anthropic 应包含 tool_use");
    let result = messages[1]["content"]
        .as_array()
        .and_then(|blocks| blocks.iter().find(|block| block["type"] == "tool_result"))
        .expect("Anthropic 应包含 tool_result");

    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[1]["role"], "user");
    assert_eq!(call["id"], TEST_CALL_ID);
    assert_eq!(call["name"], TEST_TOOL_NAME);
    if is_error {
        assert_eq!(call["input"], serde_json::json!({}));
    } else {
        assert_eq!(call["input"], serde_json::json!({ "paths": ["."] }));
    }
    assert_eq!(result["tool_use_id"], TEST_CALL_ID);
    assert_eq!(result["is_error"], is_error);
}

fn tool_output(body: &serde_json::Value, protocol: ProviderStreamProtocol) -> &str {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => body["input"]
            .as_array()
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item["type"] == "function_call_output")
            })
            .and_then(|item| item["output"].as_str()),
        ProviderStreamProtocol::OpenAiCompatible => body["messages"]
            .as_array()
            .and_then(|messages| messages.iter().find(|message| message["role"] == "tool"))
            .and_then(|message| message["content"].as_str()),
        ProviderStreamProtocol::AnthropicMessages => body["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|message| message["content"].as_array())
            .flatten()
            .find(|block| block["type"] == "tool_result")
            .and_then(|block| block["content"].as_str()),
    }
    .expect("请求体应包含对应协议的工具结果")
}

fn model_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: TEST_TOOL_NAME.to_owned(),
        description: "只读列出目录。".to_owned(),
        schema_hash: "tool-schema-list-dir".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}
