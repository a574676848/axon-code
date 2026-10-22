use super::*;
use axon_domain::{
    AxonError, ProviderError, ProviderToolCallIssue,
    RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE, RUNTIME_FAILURE_KIND_INCOMPLETE,
};

#[test]
fn parallel_openai_tool_calls_keep_distinct_ids() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            distinct_openai_tool_calls(),
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 2);
    assert_eq!(report.tool_calls[0].id, "call_read");
    assert_eq!(report.tool_calls[1].id, "call_search");
    assert!(!report.remote_raw_payload_allowed);
}

#[test]
fn duplicate_openai_tool_call_ids_fail_closed() {
    let error = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            duplicate_openai_tool_calls(),
        )
        .unwrap_err();

    assert!(format!("{error:?}").contains("provider tool-call id 重复"));
}

#[test]
fn blank_openai_tool_call_name_fails_closed() {
    let error = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            blank_name_openai_tool_call(),
        )
        .unwrap_err();

    assert_incomplete_tool_call(error, ProviderToolCallIssue::MissingName, 0);
}

#[test]
fn openai_responses_function_call_item_normalizes() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_resp_1","name":"builtin_search","arguments":"{\"query\":\"panic\"}"}}

data: {"type":"response.completed","response":{"id":"resp_1"}}"#,
            &[ModelToolDefinition {
                qualified_name: "builtin.search".to_owned(),
                description: "搜索".to_owned(),
                schema_hash: "search-schema".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}},
                    "required": ["query"],
                    "additionalProperties": false
                }),
                input_format: axon_domain::ToolInputFormat::JsonSchema,
            }],
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].id, "call_resp_1");
    assert_eq!(report.tool_calls[0].name, "builtin.search");
    assert_eq!(
        report.tool_calls[0].canonical_input(),
        r#"{"query":"panic"}"#
    );
}

#[test]
fn openai_compatible_tool_call_accepts_normalized_event_data() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_search","type":"function","function":{"name":"builtin_search","arguments":"{\"query\":\"panic\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].id, "call_search");
    assert_eq!(
        report.tool_calls[0].canonical_input(),
        r#"{"query":"panic"}"#
    );
}

#[test]
fn openai_responses_tool_call_reassembles_added_delta_done_events() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_resp_1","name":"builtin_search"}}

data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"query\":"}

data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"\"provider_stream\"}"}

data: {"type":"response.function_call_arguments.done","output_index":0}

data: {"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","call_id":"call_resp_1","name":"builtin_search","arguments":"{\"query\":\"provider_stream\"}"}}

data: {"type":"response.completed","response":{"id":"resp_1","status":"completed"}}
"#,
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].id, "call_resp_1");
    assert_eq!(
        report.tool_calls[0].canonical_input(),
        r#"{"query":"provider_stream"}"#
    );
}

#[test]
fn openai_responses_done_arguments_are_authoritative_without_output_item_done() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call_resp_done","name":"builtin_search"}}

data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"query\":\"partial"}

data: {"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"query\":\"complete\"}"}

data: {"type":"response.completed","response":{"id":"resp_done","status":"completed"}}
"#,
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].id, "call_resp_done");
    assert_eq!(
        report.tool_calls[0].canonical_input(),
        r#"{"query":"complete"}"#
    );
}

#[test]
fn openai_tool_call_keeps_name_when_later_chunk_has_blank_name() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_search","type":"function","function":{"name":"builtin_search","arguments":"{\"query\":"}}]},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"","arguments":"\"panic\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]

"#,
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].name, "builtin_search");
    assert_eq!(
        report.tool_calls[0].canonical_input(),
        r#"{"query":"panic"}"#
    );
}

#[test]
fn anthropic_tool_call_replaces_empty_input_with_json_delta() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_search","name":"builtin_search","input":{}}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"provider_stream\"}"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}

data: {"type":"message_stop"}
"#,
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].id, "toolu_search");
    assert_eq!(
        report.tool_calls[0].canonical_input(),
        r#"{"query":"provider_stream"}"#
    );
}

#[test]
fn anthropic_tool_call_replaces_complete_start_input_when_delta_repeats_full_json() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::AnthropicMessages,
            &anthropic_tool_call(
                Some("toolu_write"),
                Some("builtin_write"),
                Some(serde_json::json!({"path": "a.rs"})),
                Some(r#"{"path":"a.rs","content":"new"}"#),
            ),
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    let expected_input = serde_json::json!({"path": "a.rs", "content": "new"});
    assert_eq!(report.tool_calls[0].input.as_json(), Some(&expected_input));
}

#[test]
fn missing_openai_tool_call_id_is_incomplete() {
    let error = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            missing_id_openai_tool_call(),
        )
        .unwrap_err();

    let response = assert_incomplete_tool_call(error, ProviderToolCallIssue::MissingId, 0);
    assert_eq!(
        response.provider_protocol,
        ProviderStreamProtocol::OpenAiCompatible.as_code()
    );
}

#[test]
fn empty_openai_tool_call_arguments_are_recoverable_tool_errors() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            empty_text_arguments_openai_tool_call(),
        )
        .unwrap();

    assert!(is_recoverable_tool_call_input(&report.tool_calls[0].input));
}

#[test]
fn incomplete_openai_tool_call_arguments_are_recoverable_tool_errors() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiCompatible,
            incomplete_arguments_openai_tool_call(),
        )
        .unwrap();

    assert!(is_recoverable_tool_call_input(&report.tool_calls[0].input));
}

#[test]
fn empty_object_arguments_are_accepted_for_zero_argument_schema() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiCompatible,
            empty_arguments_openai_tool_call(),
            &[model_tool_definition(Vec::new())],
        )
        .unwrap();

    assert_eq!(report.tool_calls.len(), 1);
    assert_eq!(report.tool_calls[0].name, "mcp__gitnexus__list_repos");
    assert_eq!(report.tool_calls[0].canonical_input(), "{}");
}

fn distinct_openai_tool_calls() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_read","type":"function","function":{"name":"builtin_read_file","arguments":"{\"path\":\"Cargo.toml\"}"}},{"index":1,"id":"call_search","type":"function","function":{"name":"builtin_search","arguments":"{\"query\":\"panic\"}"}}]},"finish_reason":null}]}

data: [DONE]"#
}

fn missing_id_openai_tool_call() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"type":"function","function":{"name":"builtin_search","arguments":"{\"query\":\"panic\"}"}}]},"finish_reason":null}]}

data: [DONE]"#
}

fn incomplete_arguments_openai_tool_call() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_partial","type":"function","function":{"name":"builtin_search","arguments":"{\"query\":\"panic"}}]},"finish_reason":null}]}

data: [DONE]"#
}

fn empty_arguments_openai_tool_call() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_list_repos","type":"function","function":{"name":"mcp__gitnexus__list_repos","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]"#
}

fn empty_text_arguments_openai_tool_call() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_empty","type":"function","function":{"name":"builtin_search","arguments":""}}]},"finish_reason":"tool_calls"}]}

data: [DONE]"#
}

fn model_tool_definition(required: Vec<&str>) -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "mcp__gitnexus__list_repos".to_owned(),
        description: "列出 GitNexus 索引仓库".to_owned(),
        schema_hash: "test-schema".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": required,
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

fn duplicate_openai_tool_calls() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_same","type":"function","function":{"name":"builtin_read_file","arguments":"{\"path\":\"Cargo.toml\"}"}},{"index":1,"id":"call_same","type":"function","function":{"name":"builtin_search","arguments":"{\"query\":\"panic\"}"}}]},"finish_reason":null}]}

data: [DONE]"#
}

fn blank_name_openai_tool_call() -> &'static str {
    r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_blank","type":"function","function":{"name":"","arguments":"{\"action\":\"split\"}"}}]},"finish_reason":null}]}

data: [DONE]"#
}

fn assert_incomplete_tool_call(
    error: AxonError,
    issue: ProviderToolCallIssue,
    call_index: usize,
) -> axon_domain::ProviderInvalidResponse {
    let AxonError::Provider(ProviderError::InvalidResponse(response)) = error else {
        panic!("应返回结构化 Provider InvalidResponse，实际为 {error:?}");
    };
    assert_eq!(
        response.code,
        RUNTIME_FAILURE_CODE_PROVIDER_TOOL_CALL_INCOMPLETE
    );
    assert_eq!(response.failure_kind, RUNTIME_FAILURE_KIND_INCOMPLETE);
    assert_eq!(response.missing_field, Some(issue.as_str()));
    assert_eq!(response.call_index, Some(call_index));
    response
}

fn anthropic_tool_call(
    id: Option<&str>,
    name: Option<&str>,
    input: Option<serde_json::Value>,
    partial_json: Option<&str>,
) -> String {
    let mut content_block = serde_json::json!({"type": "tool_use"});
    if let Some(id) = id {
        content_block["id"] = serde_json::Value::String(id.to_owned());
    }
    if let Some(name) = name {
        content_block["name"] = serde_json::Value::String(name.to_owned());
    }
    if let Some(input) = input {
        content_block["input"] = input;
    }
    let mut events = vec![format!(
        "data: {}",
        serde_json::json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": content_block,
        })
    )];
    if let Some(partial_json) = partial_json {
        events.push(format!(
            "data: {}",
            serde_json::json!({
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "input_json_delta", "partial_json": partial_json},
            })
        ));
    }
    events.join("\n\n")
}

#[test]
fn anthropic_missing_id_is_incomplete() {
    let error = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::AnthropicMessages,
            &anthropic_tool_call(
                None,
                Some("builtin_search"),
                Some(serde_json::json!({"query": "axon"})),
                None,
            ),
        )
        .unwrap_err();
    let response = assert_incomplete_tool_call(error, ProviderToolCallIssue::MissingId, 0);
    assert_eq!(
        response.provider_protocol,
        ProviderStreamProtocol::AnthropicMessages.as_code()
    );
}

#[test]
fn anthropic_missing_name_is_incomplete() {
    let error = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::AnthropicMessages,
            &anthropic_tool_call(
                Some("toolu_1"),
                None,
                Some(serde_json::json!({"query": "axon"})),
                None,
            ),
        )
        .unwrap_err();
    assert_incomplete_tool_call(error, ProviderToolCallIssue::MissingName, 0);
}

#[test]
fn anthropic_empty_and_malformed_arguments_are_recoverable_tool_errors() {
    let empty = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::AnthropicMessages,
            &anthropic_tool_call(Some("toolu_1"), Some("builtin_search"), None, None),
        )
        .unwrap();
    assert!(is_recoverable_tool_call_input(&empty.tool_calls[0].input));

    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::AnthropicMessages,
            &anthropic_tool_call(
                Some("toolu_1"),
                Some("builtin_search"),
                Some(serde_json::json!({})),
                Some("{\"query\":"),
            ),
        )
        .unwrap();

    assert!(is_recoverable_tool_call_input(&report.tool_calls[0].input));
}

#[test]
fn anthropic_empty_object_stays_json_before_registry_validation() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::AnthropicMessages,
            &anthropic_tool_call(
                Some("toolu_1"),
                Some("mcp__gitnexus__list_repos"),
                Some(serde_json::json!({})),
                None,
            ),
            &[model_tool_definition(vec!["query"])],
        )
        .unwrap();

    assert_eq!(report.tool_calls[0].canonical_input(), "{}");
    assert!(!is_recoverable_tool_call_input(&report.tool_calls[0].input));
}

#[test]
fn responses_custom_tool_call_preserves_freeform_text() {
    let report = ProviderToolCallRuntime
        .normalize(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","call_id":"call_edit","name":"builtin.edit"}}

data: {"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"*** Begin Patch\n"}

data: {"type":"response.custom_tool_call_input.done","output_index":0,"input":"*** Begin Patch\n*** End Patch"}

data: {"type":"response.completed","response":{"id":"resp_edit","status":"completed"}}"#,
        )
        .unwrap();

    assert_eq!(
        report.tool_calls[0].input.as_text(),
        Some("*** Begin Patch\n*** End Patch")
    );
}

#[test]
fn function_fallback_restores_freeform_text() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_edit","type":"function","function":{"name":"builtin.edit","arguments":"{\"input\":\"*** Begin Patch\\n*** End Patch\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]"#,
            &[freeform_model_tool_definition()],
        )
        .unwrap();

    assert_eq!(
        report.tool_calls[0].input.as_text(),
        Some("*** Begin Patch\n*** End Patch")
    );
}

#[test]
fn function_fallback_accepts_code_edit_alias_for_freeform_edit() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_edit","type":"function","function":{"name":"builtin.edit","arguments":"{\"code_edit\":\"*** Begin Patch\\n*** End Patch\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]"#,
            &[freeform_model_tool_definition()],
        )
        .unwrap();

    assert_eq!(
        report.tool_calls[0].input.as_text(),
        Some("*** Begin Patch\n*** End Patch")
    );
}

#[test]
fn responses_custom_apply_patch_restores_internal_edit_name() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.done","output_index":0,"item":{"type":"custom_tool_call","call_id":"call_edit","name":"apply_patch","input":"*** Begin Patch\n*** End Patch"}}

data: {"type":"response.completed","response":{"id":"resp_edit"}}"#,
            &[freeform_model_tool_definition()],
        )
        .unwrap();

    assert_eq!(report.tool_calls[0].name, "builtin.edit");
    assert_eq!(
        report.tool_calls[0].input.as_text(),
        Some("*** Begin Patch\n*** End Patch")
    );
}

fn freeform_model_tool_definition() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "builtin.edit".to_owned(),
        description: "编辑文件".to_owned(),
        schema_hash: "freeform-test-schema".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"input": {"type": "string"}},
            "required": ["input"],
            "additionalProperties": false
        }),
        input_format: axon_domain::ToolInputFormat::Freeform {
            fallback_field: "input".to_owned(),
            format: axon_domain::ToolFreeformFormat {
                format_type: "grammar".to_owned(),
                syntax: "lark".to_owned(),
                definition: "start: /[\\s\\S]+/".to_owned(),
            },
        },
    }
}

#[test]
fn empty_object_arguments_are_preserved_for_scheduler_validation() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiCompatible,
            empty_arguments_openai_tool_call(),
            &[model_tool_definition(vec!["query"])],
        )
        .unwrap();

    assert_eq!(
        report.tool_calls[0].input.as_json(),
        Some(&serde_json::json!({}))
    );
}
