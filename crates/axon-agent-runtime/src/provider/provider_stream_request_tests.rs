use super::*;
use crate::PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL;
use crate::ProviderVisibleCacheBreakReason;
use crate::ProviderVisibleCacheShape;
use crate::{
    AgentModelMessage, ProviderStructuredOutputContract, ProviderStructuredOutputMode,
    ProviderToolCall,
};

#[test]
fn openai_request_enables_parallel_tool_calls_when_tools_exist() {
    let plan = ProviderStreamingHttpRequestRuntime
        .plan(request_input(vec![model_tool()]))
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["parallel_tool_calls"], true);
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert!(!plan.remote_request_body_allowed);
}

#[test]
fn openai_request_disables_parallel_tool_calls_without_tools() {
    let plan = ProviderStreamingHttpRequestRuntime
        .plan(request_input(Vec::new()))
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["parallel_tool_calls"], false);
    assert!(body.get("tool_choice").is_none());
    assert_eq!(body["tools"].as_array().unwrap().len(), 0);
}

#[test]
fn provider_tool_envelopes_preserve_complete_input_schema() {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "options": {
                "type": "object",
                "description": "查询选项",
                "properties": {
                    "mode": {
                        "type": "string",
                        "description": "执行模式",
                        "enum": ["fast", "safe"]
                    }
                },
                "required": ["mode"]
            },
            "tags": {
                "type": "array",
                "description": "标签列表",
                "items": {"type": "string"}
            }
        },
        "required": ["options"],
        "additionalProperties": false
    });
    let tool = ModelToolDefinition {
        qualified_name: "mcp__demo__query".to_owned(),
        description: "查询演示数据".to_owned(),
        schema_hash: "nested-schema".to_owned(),
        input_schema: schema.clone(),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    };

    for (protocol, pointer) in [
        (
            ProviderStreamProtocol::OpenAiCompatible,
            "/tools/0/function/parameters",
        ),
        (
            ProviderStreamProtocol::OpenAiResponses,
            "/tools/0/parameters",
        ),
        (
            ProviderStreamProtocol::AnthropicMessages,
            "/tools/0/input_schema",
        ),
    ] {
        let mut input = request_input(vec![tool.clone()]);
        input.protocol = protocol;
        refresh_prompt_cache_plan(&mut input);
        let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

        assert_eq!(body.pointer(pointer), Some(&schema));
    }
}

#[test]
fn update_plan_contract_reaches_every_provider_tool_envelope() {
    let mut registry = crate::ToolRegistry::new();
    crate::AgentToolRegistryRuntime::register_builtin_tools(&mut registry)
        .expect("内置工具应注册成功");
    let tool = registry
        .model_tools_for_provider()
        .into_iter()
        .find(|tool| tool.qualified_name == crate::UPDATE_PLAN_TOOL_QUALIFIED_NAME)
        .expect("update_plan 必须对 Provider 可见");
    let expected_name = crate::provider_tool_name(&tool.qualified_name);

    for (protocol, tool_pointer, name_pointer, schema_pointer) in [
        (
            ProviderStreamProtocol::OpenAiCompatible,
            "/tools/0/function",
            "/tools/0/function/name",
            "/tools/0/function/parameters",
        ),
        (
            ProviderStreamProtocol::OpenAiResponses,
            "/tools/0",
            "/tools/0/name",
            "/tools/0/parameters",
        ),
        (
            ProviderStreamProtocol::AnthropicMessages,
            "/tools/0",
            "/tools/0/name",
            "/tools/0/input_schema",
        ),
    ] {
        let mut input = request_input(vec![tool.clone()]);
        input.protocol = protocol;
        refresh_prompt_cache_plan(&mut input);
        let plan = ProviderStreamingHttpRequestRuntime
            .plan(input)
            .expect("Provider 请求应构建成功");
        let body: serde_json::Value =
            serde_json::from_str(&plan.body_text).expect("Provider 请求应为 JSON");
        let envelope = body.pointer(tool_pointer).expect("工具 envelope 应存在");

        assert_eq!(
            body.pointer(name_pointer),
            Some(&serde_json::json!(expected_name))
        );
        assert_eq!(
            envelope.get("description"),
            Some(&serde_json::json!(tool.description))
        );
        assert_eq!(body.pointer(schema_pointer), Some(&tool.input_schema));
        assert_eq!(
            body.pointer(&format!("{schema_pointer}/properties/plan/minItems")),
            Some(&serde_json::json!(crate::UPDATE_PLAN_MIN_STEPS))
        );
        assert_eq!(
            body.pointer(&format!(
                "{schema_pointer}/properties/plan/items/allOf/0/then/required/0"
            )),
            Some(&serde_json::json!("verification"))
        );
    }
}

#[test]
fn openai_responses_request_uses_responses_path_and_input_items() {
    let mut input = request_input(vec![model_tool()]);
    input.protocol = ProviderStreamProtocol::OpenAiResponses;
    input.capabilities =
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI);
    input.reasoning_effort = Some("high".to_owned());
    input
        .messages
        .insert(0, AgentModelMessage::text("system", "系统指令"));
    input
        .messages
        .insert(1, AgentModelMessage::text("developer", "开发者约束"));
    input.prompt_cache_plan = request_prompt_cache_plan(&input);
    let expected_prompt_cache_key = input.prompt_cache_plan.namespace_key.clone();
    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert!(plan.url.ends_with(OPENAI_RESPONSES_PATH));
    assert_eq!(body["instructions"], "系统指令");
    assert_eq!(body["parallel_tool_calls"], true);
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["store"], false);
    assert_eq!(body["stream"], true);
    assert_eq!(body["prompt_cache_key"], expected_prompt_cache_key);
    assert!(body.get("text").is_none());
    assert_eq!(body["input"][0]["type"], "message");
    assert_eq!(body["input"][0]["role"], "developer");
    assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(body["input"][1]["role"], "user");
    assert_eq!(body["tools"][0]["type"], "function");
    assert_eq!(body["reasoning"]["effort"], "high");
    assert_eq!(body["reasoning"]["summary"], REASONING_SUMMARY_AUTO);
}

#[test]
fn provider_capability_combinations_gate_cache_state_and_protocol_fields() {
    let mut server_state = request_input(Vec::new());
    server_state.protocol = ProviderStreamProtocol::OpenAiResponses;
    server_state.capabilities =
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI);
    server_state.server_conversation_state_ref = Some("resp-1".to_owned());
    refresh_prompt_cache_plan(&mut server_state);
    let server_plan = ProviderStreamingHttpRequestRuntime
        .plan(server_state)
        .unwrap();
    let server_body: serde_json::Value = serde_json::from_str(&server_plan.body_text).unwrap();
    assert!(server_plan.server_conversation_state_allowed);
    assert!(server_body["prompt_cache_key"].is_string());
    assert_eq!(server_body["previous_response_id"], "resp-1");

    let mut prompt_only = request_input(Vec::new());
    prompt_only.protocol = ProviderStreamProtocol::OpenAiResponses;
    prompt_only.capabilities = axon_config::ProviderCapabilities {
        supports_prompt_cache: true,
        supports_explicit_cache_breakpoint: false,
        supports_server_conversation_state: false,
    };
    prompt_only.server_conversation_state_ref = Some("ignored".to_owned());
    prompt_only.prompt_cache_plan = request_prompt_cache_plan(&prompt_only);
    let prompt_plan = ProviderStreamingHttpRequestRuntime
        .plan(prompt_only)
        .unwrap();
    let prompt_body: serde_json::Value = serde_json::from_str(&prompt_plan.body_text).unwrap();
    assert!(!prompt_plan.server_conversation_state_allowed);
    assert!(prompt_body["prompt_cache_key"].is_string());
    assert!(prompt_body.get("previous_response_id").is_none());

    let mut stateless = request_input(Vec::new());
    stateless.protocol = ProviderStreamProtocol::OpenAiResponses;
    stateless.capabilities = axon_config::ProviderCapabilities::stateless();
    stateless.server_conversation_state_ref = Some("ignored".to_owned());
    stateless.prompt_cache_plan = request_prompt_cache_plan(&stateless);
    let stateless_body: serde_json::Value = serde_json::from_str(
        &ProviderStreamingHttpRequestRuntime
            .plan(stateless)
            .unwrap()
            .body_text,
    )
    .unwrap();
    assert!(stateless_body.get("prompt_cache_key").is_none());
    assert!(stateless_body.get("previous_response_id").is_none());

    let mut anthropic = request_input(Vec::new());
    anthropic.protocol = ProviderStreamProtocol::AnthropicMessages;
    anthropic.capabilities = axon_config::ProviderCapabilities {
        supports_prompt_cache: true,
        supports_explicit_cache_breakpoint: false,
        supports_server_conversation_state: false,
    };
    anthropic.prompt_cache_plan = request_prompt_cache_plan(&anthropic);
    let anthropic_body: serde_json::Value = serde_json::from_str(
        &ProviderStreamingHttpRequestRuntime
            .plan(anthropic)
            .unwrap()
            .body_text,
    )
    .unwrap();
    assert!(
        anthropic_body["system"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("cache_control").is_none())
    );
}

#[test]
fn openai_responses_request_only_includes_encrypted_reasoning_content() {
    let mut without_reasoning = request_input(Vec::new());
    without_reasoning.protocol = ProviderStreamProtocol::OpenAiResponses;
    refresh_prompt_cache_plan(&mut without_reasoning);
    let plan = ProviderStreamingHttpRequestRuntime
        .plan(without_reasoning)
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    assert_eq!(body["include"], serde_json::json!([]));

    let mut with_reasoning = request_input(Vec::new());
    with_reasoning.protocol = ProviderStreamProtocol::OpenAiResponses;
    with_reasoning.reasoning_effort = Some("high".to_owned());
    refresh_prompt_cache_plan(&mut with_reasoning);
    let plan = ProviderStreamingHttpRequestRuntime
        .plan(with_reasoning)
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    assert_eq!(
        body["include"],
        serde_json::json!([OPENAI_REASONING_INCLUDE_ENCRYPTED_CONTENT])
    );
}

#[test]
fn openai_responses_request_replays_function_call_before_function_call_output() {
    let mut input = request_input(vec![model_tool()]);
    input.protocol = ProviderStreamProtocol::OpenAiResponses;
    *input.messages = crate::agent_run_tool_result_messages(
        &[ProviderToolCall::new(
            "call_search",
            "builtin_search",
            "{\"query\":\"provider_stream\"}",
        )],
        &[crate::AgentRunToolResultReceipt {
            provider_call_id: "call_search".to_owned(),
            tool_name: "builtin.search".to_owned(),
            status: "completed".to_owned(),
            summary: "search query=provider_stream".to_owned(),
            metadata: None,
            output_ref: None,
            error_type: None,
            remote_trigger_allowed: false,
            filesystem_write_performed: false,
        }],
        None,
        crate::AgentRunToolOutputPolicy::default(),
    )
    .unwrap();
    refresh_prompt_cache_plan(&mut input);
    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["input"][0]["type"], "function_call");
    assert_eq!(body["input"][0]["call_id"], "call_search");
    assert_eq!(body["input"][0]["name"], "builtin_search");
    assert_eq!(body["input"][0]["status"], "completed");
    assert_eq!(body["input"][1]["type"], "function_call_output");
    assert_eq!(body["input"][1]["call_id"], "call_search");
    assert!(body["input"][1].get("item_reference").is_none());
}

#[test]
fn openai_responses_request_preserves_assistant_text_before_function_call() {
    let mut input = request_input(vec![model_tool()]);
    input.protocol = ProviderStreamProtocol::OpenAiResponses;
    *input.messages = vec![
        AgentModelMessage::text("assistant", "我先查询资料").with_tool_calls(vec![
            crate::AgentModelToolCallMetadata {
                id: "call_search".to_owned(),
                name: "builtin_search".to_owned(),
                input: r#"{"query":"provider"}"#.into(),
            },
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);
    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert_eq!(body["input"][0]["type"], "message");
    assert_eq!(body["input"][0]["role"], "assistant");
    assert_eq!(body["input"][0]["content"][0]["type"], "output_text");
    assert_eq!(body["input"][0]["content"][0]["text"], "我先查询资料");
    assert_eq!(body["input"][1]["type"], "function_call");
    assert_eq!(body["input"][1]["call_id"], "call_search");
    assert_eq!(body["input"][1]["name"], "builtin_search");
}

#[test]
fn openai_chat_request_emits_reasoning_fields() {
    let mut input = request_input(Vec::new());
    input.reasoning_effort = Some("medium".to_owned());
    input.reasoning_summary = Some("auto".to_owned());
    *input.messages = vec![
        AgentModelMessage::text("assistant", "中间回答")
            .with_openai_reasoning_content("xiaomi-reasoning"),
    ];
    refresh_prompt_cache_plan(&mut input);
    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

    assert!(body.get("enable_thinking").is_none());
    assert_eq!(body["reasoning_effort"], "medium");
    /*
     * `reasoning_summary` 是 OpenAI Responses 的字段，Chat Completions 协议里没有它；
     * 严格校验的 Provider 会直接 400（真机回执：UNKNOWN_FIELD 未知请求字段
     * reasoning_summary），因此这里刻意不发，推理正文走 `reasoning_content`。
     */
    assert!(body.get("reasoning_summary").is_none(), "{body}");
    assert_eq!(body["messages"][0]["reasoning_content"], "xiaomi-reasoning");
}

#[test]
fn provider_request_projects_file_as_bounded_preview_with_references() {
    let mut input = request_input(Vec::new());
    *input.messages = vec![
        AgentModelMessage::text("user", "读文件").with_local_files(vec![
            crate::AgentModelLocalFileRef {
                name: "README.md".to_owned(),
                path: Some("C:\\work\\README.md".to_owned()),
                mime_type: "text/markdown".to_owned(),
                preview: "# 预览内容".to_owned(),
                byte_len: Some(512),
                sha256: Some("a".repeat(64)),
                archive_ref: Some("blobs/archive.blob".to_owned()),
                source_ref: Some("local-upload://session/0".to_owned()),
            },
        ]),
    ];
    refresh_prompt_cache_plan(&mut input);

    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    let body_text = body.to_string();
    assert!(body_text.contains("预览内容"));
    assert!(body_text.contains("archiveRef=blobs/archive.blob"));
    assert!(body_text.contains("sourceRef=local-upload://session/0"));
}

#[test]
fn anthropic_request_preserves_xhigh_and_max_with_adaptive_thinking() {
    for effort in ["xhigh", "max"] {
        let mut input = request_input(Vec::new());
        input.protocol = ProviderStreamProtocol::AnthropicMessages;
        input.model_id = "claude-test".to_owned();
        input.reasoning_effort = Some(effort.to_owned());
        refresh_prompt_cache_plan(&mut input);
        let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();

        assert_eq!(body["thinking"]["type"], ANTHROPIC_THINKING_TYPE_ADAPTIVE);
        assert_eq!(
            body["thinking"]["display"],
            ANTHROPIC_THINKING_DISPLAY_SUMMARIZED
        );
        assert_eq!(body["output_config"]["effort"], effort);
        assert!(body["thinking"].get("budget_tokens").is_none());
    }
}

#[test]
fn anthropic_request_marks_last_tool_schema_for_prompt_cache() {
    let mut input = request_input(vec![model_tool(), search_tool()]);
    input.protocol = ProviderStreamProtocol::AnthropicMessages;
    input.model_id = "claude-test".to_owned();
    refresh_prompt_cache_plan(&mut input);
    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    let tools = body["tools"].as_array().unwrap();

    assert_eq!(tools.len(), 2);
    assert!(tools[0].get("cache_control").is_none());
    assert_eq!(
        tools[1]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert_eq!(tools[1]["name"], "builtin_search");
    assert!(body.get("thinking").is_none());
    assert!(body.get("output_config").is_none());
}

#[test]
fn anthropic_request_marks_only_last_system_prompt_for_prompt_cache() {
    let mut input = request_input(Vec::new());
    input.protocol = ProviderStreamProtocol::AnthropicMessages;
    input.model_id = "claude-test".to_owned();
    *input.messages = vec![
        AgentModelMessage::text("system", "stable system"),
        AgentModelMessage::text("system", "durable context"),
        AgentModelMessage::text("user", "latest user"),
    ];
    refresh_prompt_cache_plan(&mut input);
    let plan = ProviderStreamingHttpRequestRuntime.plan(input).unwrap();
    let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
    let system = body["system"].as_array().unwrap();

    assert_eq!(system[0]["text"], "stable system");
    assert!(system[0].get("cache_control").is_none());
    assert_eq!(system[1]["text"], "durable context");
    assert_eq!(
        system[1]["cache_control"]["type"],
        PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
    assert!(
        body["messages"][0]["content"][0]
            .get("cache_control")
            .is_some()
    );
}

#[test]
fn provider_reasoning_effort_validation_matches_protocol_capabilities() {
    for effort in ["none", "minimal", "xhigh", "max"] {
        let mut input = request_input(Vec::new());
        input.reasoning_effort = Some(effort.to_owned());
        assert!(ProviderStreamingHttpRequestRuntime.plan(input).is_ok());
    }
    for effort in ["ultra", "think-high"] {
        let mut input = request_input(Vec::new());
        input.reasoning_effort = Some(effort.to_owned());
        assert!(matches!(
            ProviderStreamingHttpRequestRuntime.plan(input),
            Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidInput(
                _
            )))
        ));
    }
    for effort in ["low", "xhigh", "max"] {
        let mut input = request_input(Vec::new());
        input.protocol = ProviderStreamProtocol::AnthropicMessages;
        input.reasoning_effort = Some(effort.to_owned());
        assert!(ProviderStreamingHttpRequestRuntime.plan(input).is_ok());
    }
    for effort in ["none", "minimal", "ultra"] {
        let mut input = request_input(Vec::new());
        input.protocol = ProviderStreamProtocol::AnthropicMessages;
        input.reasoning_effort = Some(effort.to_owned());
        assert!(matches!(
            ProviderStreamingHttpRequestRuntime.plan(input),
            Err(AxonError::Runtime(axon_domain::RuntimeError::InvalidInput(
                _
            )))
        ));
    }
}

#[test]
fn openai_request_service_tier_policy_matches_provider_support() {
    let mut priority_input = request_input(Vec::new());
    priority_input.service_tier = Some("priority".to_owned());
    let priority_plan = ProviderStreamingHttpRequestRuntime
        .plan(priority_input)
        .unwrap();
    let priority_body: serde_json::Value = serde_json::from_str(&priority_plan.body_text).unwrap();
    assert_eq!(priority_body["service_tier"], "priority");

    let mut default_input = request_input(Vec::new());
    default_input.service_tier = Some(SERVICE_TIER_DEFAULT.to_owned());
    let default_plan = ProviderStreamingHttpRequestRuntime
        .plan(default_input)
        .unwrap();
    let default_body: serde_json::Value = serde_json::from_str(&default_plan.body_text).unwrap();
    assert!(default_body.get("service_tier").is_none());
}

#[test]
fn cache_shape_hashes_all_provider_visible_options() {
    let mut anthropic = request_input(Vec::new());
    anthropic.protocol = ProviderStreamProtocol::AnthropicMessages;
    anthropic.reasoning_effort = Some("low".to_owned());
    refresh_prompt_cache_plan(&mut anthropic);
    let first_plan = ProviderStreamingHttpRequestRuntime
        .plan(anthropic.clone())
        .unwrap();
    let first_shape = ProviderVisibleCacheShape::from_serialized_request(
        anthropic.protocol,
        &first_plan.body_text,
    )
    .unwrap();

    anthropic.reasoning_effort = None;
    refresh_prompt_cache_plan(&mut anthropic);
    let second_plan = ProviderStreamingHttpRequestRuntime
        .plan(anthropic.clone())
        .unwrap();
    let second_shape = ProviderVisibleCacheShape::from_serialized_request(
        anthropic.protocol,
        &second_plan.body_text,
    )
    .unwrap();
    assert_ne!(first_shape.options_hash, second_shape.options_hash);

    anthropic.reasoning_effort = Some("low".to_owned());
    anthropic.structured_output = Some(ProviderStructuredOutputContract {
        schema: serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}}
        }),
        mode: ProviderStructuredOutputMode::JsonSchema,
    });
    refresh_prompt_cache_plan(&mut anthropic);
    let structured_plan = ProviderStreamingHttpRequestRuntime
        .plan(anthropic.clone())
        .unwrap();
    let structured_shape = ProviderVisibleCacheShape::from_serialized_request(
        anthropic.protocol,
        &structured_plan.body_text,
    )
    .unwrap();
    assert_ne!(first_shape.options_hash, structured_shape.options_hash);
}

#[test]
fn cache_shape_marks_same_length_prefix_rewrite() {
    let mut first = request_input(Vec::new());
    first.messages = vec![
        AgentModelMessage::text("user", "旧任务"),
        AgentModelMessage::text("assistant", "旧回答"),
    ]
    .into();
    refresh_prompt_cache_plan(&mut first);
    let first_plan = ProviderStreamingHttpRequestRuntime
        .plan(first.clone())
        .unwrap();
    let first_shape =
        ProviderVisibleCacheShape::from_serialized_request(first.protocol, &first_plan.body_text)
            .unwrap();

    first.messages = vec![
        AgentModelMessage::text("user", "新任务"),
        AgentModelMessage::text("assistant", "新回答"),
    ]
    .into();
    refresh_prompt_cache_plan(&mut first);
    let second_plan = ProviderStreamingHttpRequestRuntime
        .plan(first.clone())
        .unwrap();
    let second_shape =
        ProviderVisibleCacheShape::from_serialized_request(first.protocol, &second_plan.body_text)
            .unwrap();
    let comparison = second_shape.compare(Some(&first_shape));
    assert!(comparison.conversation_rewritten);
    assert!(comparison.message_prefix_changed);
    assert_eq!(
        comparison.reason,
        ProviderVisibleCacheBreakReason::MessagePrefixChanged
    );
}
