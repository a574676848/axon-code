use super::*;

#[test]
fn openai_chat_emits_cumulative_arguments() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    let first = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call-1","function":{"name":"edit","arguments":"{\"input\":\"src/"}}]}}]}"#;
    let second = r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"lib.rs\"}"}}]}}]}"#;

    let first = runtime
        .consume_line(ProviderStreamProtocol::OpenAiCompatible, first)
        .unwrap();
    let second = runtime
        .consume_line(ProviderStreamProtocol::OpenAiCompatible, second)
        .unwrap();

    assert_eq!(first[0].input_text, r#"{"input":"src/"#);
    assert_eq!(first[0].input_delta, r#"{"input":"src/"#);
    assert!(!first[0].input_reset);
    assert_eq!(second[0].input_text, r#"{"input":"src/lib.rs"}"#);
    assert_eq!(second[0].input_delta, r#"lib.rs"}"#);
}

#[test]
fn responses_done_event_replaces_accumulated_arguments() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","call_id":"call-1","name":"edit"}}"#,
        )
        .unwrap();
    runtime
        .consume_line(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"input\":\"partial\"}"}"#,
        )
        .unwrap();
    let partial = runtime
        .consume_line(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"input\":\"complete\"}"}"#,
        )
        .unwrap();

    assert_eq!(partial[0].input_text, r#"{"input":"complete"}"#);
    assert!(partial[0].input_reset);
    assert_eq!(partial[0].wire_kind, ProviderToolCallWireKind::Function);
    let calls = runtime
        .finish(ProviderStreamProtocol::OpenAiResponses, &[])
        .unwrap();
    assert_eq!(
        calls[0].input.as_json(),
        Some(&serde_json::json!({"input": "complete"}))
    );
}

#[test]
fn responses_custom_tool_done_event_replaces_accumulated_input() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","call_id":"call-1","name":"edit"}}"#,
        )
        .unwrap();
    runtime
        .consume_line(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"partial"}"#,
        )
        .unwrap();
    let partial = runtime
        .consume_line(
            ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.custom_tool_call_input.done","output_index":0,"input":"complete"}"#,
        )
        .unwrap();

    assert_eq!(partial[0].input_text, "complete");
    assert!(partial[0].input_reset);
    assert_eq!(partial[0].wire_kind, ProviderToolCallWireKind::Custom);
    let calls = runtime
        .finish(ProviderStreamProtocol::OpenAiResponses, &[])
        .unwrap();
    assert_eq!(calls[0].input.as_text(), Some("complete"));
}

#[test]
fn anthropic_emits_partial_json_after_tool_start() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool-1","name":"edit","input":{}}}"#,
        )
        .unwrap();
    let partial = runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"a.rs\"}"}}"#,
        )
        .unwrap();

    assert_eq!(partial[0].id, "tool-1");
    assert_eq!(partial[0].input_text, r#"{"input":"a.rs"}"#);
}

#[test]
fn anthropic_uses_complete_input_when_provider_omits_json_deltas() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"edit","input":{"input":"new"}}}"#,
        )
        .unwrap();

    let calls = runtime
        .finish(ProviderStreamProtocol::AnthropicMessages, &[])
        .unwrap();

    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].input.as_json(),
        Some(&serde_json::json!({"input": "new"}))
    );
}

#[test]
fn anthropic_replaces_full_start_input_when_delta_repeats_complete_json() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-1","name":"edit","input":{"input":"old"}}}"#,
        )
        .unwrap();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"new\"}"}}"#,
        )
        .unwrap();

    let calls = runtime
        .finish(ProviderStreamProtocol::AnthropicMessages, &[])
        .unwrap();

    assert_eq!(
        calls[0].input.as_json(),
        Some(&serde_json::json!({"input": "new"}))
    );
}

#[test]
fn anthropic_replaces_incomplete_arguments_when_delta_restarts_full_json() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool-2","name":"edit","input":{}}}"#,
        )
        .unwrap();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"a"}}"#,
        )
        .unwrap();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"new\"}"}}"#,
        )
        .unwrap();

    let calls = runtime
        .finish(ProviderStreamProtocol::AnthropicMessages, &[])
        .unwrap();

    assert_eq!(
        calls[0].input.as_json(),
        Some(&serde_json::json!({"input": "new"}))
    );
}

#[test]
fn anthropic_malformed_event_json_is_provider_failure() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    let error = runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            "data: {\"type\":\"content_block_delta\"",
        )
        .unwrap_err();

    assert!(matches!(
        error,
        AxonError::Provider(axon_domain::ProviderError::InvalidResponse(_))
    ));
}

#[test]
fn anthropic_preserves_braces_in_split_command_and_regex_deltas() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-sh","name":"builtin_shell_command","input":{}}}"#,
        )
        .unwrap();
    runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"1..3 | % "}}"#,
        )
        .unwrap();
    let brace_chunk = runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{ $n++ }; rg 'a"}}"#,
        )
        .unwrap();
    assert!(!brace_chunk[0].input_reset);
    assert_eq!(brace_chunk[0].input_delta, "{ $n++ }; rg 'a");
    let regex_chunk = runtime
        .consume_line(
            ProviderStreamProtocol::AnthropicMessages,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{1,3}'\",\"intent\":\"统计\"}"}}"#,
        )
        .unwrap();
    assert!(!regex_chunk[0].input_reset);

    let calls = runtime
        .finish(ProviderStreamProtocol::AnthropicMessages, &[])
        .unwrap();
    assert_eq!(
        calls[0].input.as_json(),
        Some(&serde_json::json!({
            "command": "1..3 | % { $n++ }; rg 'a{1,3}'",
            "intent": "统计"
        }))
    );
}

#[test]
fn anthropic_preserves_nested_json_object_across_deltas() {
    let mut runtime = ProviderToolCallStreamRuntime::default();
    for line in [
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool-nested","name":"builtin_search","input":{}}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"meta\":"}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"a\":1}"}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":",\"ok\":true}"}}"#,
    ] {
        runtime
            .consume_line(ProviderStreamProtocol::AnthropicMessages, line)
            .unwrap();
    }

    let calls = runtime
        .finish(ProviderStreamProtocol::AnthropicMessages, &[])
        .unwrap();
    assert_eq!(
        calls[0].input.as_json(),
        Some(&serde_json::json!({ "meta": { "a": 1 }, "ok": true }))
    );
}

#[test]
fn anthropic_empty_delta_and_omitted_input_respect_tool_schema() {
    let tools = vec![
        ModelToolDefinition {
            qualified_name: "builtin.get_context_remaining".to_owned(),
            description: "查询剩余预算".to_owned(),
            schema_hash: "budget-schema".to_owned(),
            input_schema: serde_json::json!({"type": "object", "properties": {}, "required": []}),
            input_format: axon_domain::ToolInputFormat::JsonSchema,
        },
        ModelToolDefinition {
            qualified_name: "builtin.search".to_owned(),
            description: "搜索".to_owned(),
            schema_hash: "search-schema".to_owned(),
            input_schema: serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            input_format: axon_domain::ToolInputFormat::JsonSchema,
        },
    ];
    let mut runtime = ProviderToolCallStreamRuntime::default();
    for line in [
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"t0","name":"builtin_get_context_remaining","input":{}}}"#,
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":""}}"#,
        r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"t1","name":"builtin_get_context_remaining"}}"#,
        r#"data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"t2","name":"builtin_search","input":null}}"#,
        r#"data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":""}}"#,
    ] {
        runtime
            .consume_line(ProviderStreamProtocol::AnthropicMessages, line)
            .unwrap();
    }

    let calls = runtime
        .finish(ProviderStreamProtocol::AnthropicMessages, &tools)
        .unwrap();
    assert_eq!(calls[0].name, "builtin.get_context_remaining");
    assert_eq!(calls[0].input.as_json(), Some(&serde_json::json!({})));
    assert_eq!(calls[1].name, "builtin.get_context_remaining");
    assert_eq!(calls[1].input.as_json(), Some(&serde_json::json!({})));
    assert_eq!(calls[2].name, "builtin.search");
    assert!(crate::is_recoverable_tool_call_input(&calls[2].input));
}
