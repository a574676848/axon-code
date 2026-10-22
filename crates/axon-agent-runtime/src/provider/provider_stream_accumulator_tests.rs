use super::*;

const OPENAI_RESPONSES_CONTINUE_EVENT: &str =
    r#"{"type":"response.completed","response":{"end_turn":false}}"#;
const OPENAI_CHAT_STOP_EVENT: &str = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
const ANTHROPIC_TOOL_USE_EVENT: &str =
    r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#;

#[test]
fn text_function_calls_are_structured_without_visible_xml() {
    let model_tools = vec![ModelToolDefinition {
        qualified_name: "builtin.invoke_skill".to_owned(),
        description: "加载 Skill".to_owned(),
        schema_hash: "test-schema".to_owned(),
        input_schema: serde_json::json!({"type": "object"}),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }];
    let mut accumulator =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiCompatible, model_tools);

    let first = openai_text_event("准备调用\n<function_", None);
    let second = openai_text_event(
        "calls>\n<function name=\"builtin_invoke_skill\">\n<args_json>{\"skill_name\":\"operate-axon\"}</args_json>\n</function>\n",
        None,
    );
    let third = openai_text_event("</function_calls>\n", Some("stop"));

    let projected = [
        accumulator.consume_projected(&first),
        accumulator.consume_projected(&second),
        accumulator.consume_projected(&third),
    ]
    .concat();
    let stream = accumulator.finish().unwrap();

    assert_eq!(
        projected
            .iter()
            .map(|delta| delta.content_delta.as_str())
            .collect::<String>(),
        "准备调用\n"
    );
    assert_eq!(
        stream
            .deltas
            .iter()
            .map(|delta| delta.content_delta.as_str())
            .collect::<String>(),
        "准备调用\n"
    );
    assert_eq!(stream.end_turn, Some(false));
    assert_eq!(stream.tool_calls.len(), 1);
    assert_eq!(stream.tool_calls[0].name, "builtin.invoke_skill");
    assert_eq!(
        stream.tool_calls[0].input.as_json(),
        Some(&serde_json::json!({"skill_name": "operate-axon"}))
    );
}

#[test]
fn text_function_calls_inside_fenced_code_remain_visible() {
    let content = "```xml\n<function_calls>\n<function name=\"builtin_invoke_skill\">\n<args_json>{\"skill_name\":\"operate-axon\"}</args_json>\n</function_calls>\n```\n";
    let mut accumulator =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiCompatible, Vec::new());
    accumulator.consume(&openai_text_event(content, Some("stop")));

    let stream = accumulator.finish().unwrap();

    assert_eq!(
        stream
            .deltas
            .iter()
            .map(|delta| delta.content_delta.as_str())
            .collect::<String>(),
        content
    );
    assert!(stream.tool_calls.is_empty());
    assert_eq!(stream.end_turn, Some(true));
}

fn openai_text_event(content: &str, finish_reason: Option<&str>) -> String {
    serde_json::json!({
        "choices": [{
            "delta": {"content": content},
            "finish_reason": finish_reason,
        }]
    })
    .to_string()
}

#[test]
fn empty_provider_stream_is_rejected() {
    let accumulator =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiResponses, Vec::new());

    let error = accumulator.finish().unwrap_err();

    assert!(format!("{error:?}").contains(EMPTY_PROVIDER_STREAM_ERROR));
}

#[test]
fn stream_protocols_map_finish_and_stop_reasons_to_end_turn() {
    let mut responses =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiResponses, Vec::new());
    responses.consume(OPENAI_RESPONSES_CONTINUE_EVENT);
    assert_eq!(responses.finish().unwrap().end_turn, Some(false));

    let mut chat =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiCompatible, Vec::new());
    chat.consume(OPENAI_CHAT_STOP_EVENT);
    assert_eq!(chat.finish().unwrap().end_turn, Some(true));

    let mut anthropic =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::AnthropicMessages, Vec::new());
    anthropic.consume(ANTHROPIC_TOOL_USE_EVENT);
    assert_eq!(anthropic.finish().unwrap().end_turn, Some(false));
}

#[test]
fn anthropic_truncated_tool_arguments_remain_a_completed_tool_turn() {
    let mut accumulator =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::AnthropicMessages, Vec::new());
    accumulator.consume(
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"builtin_apply_diff","input":{}}}"#,
    );
    accumulator.consume(
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"E:\\\\zbg\\\\audit.md\""}}"#,
    );
    accumulator.consume(r#"{"type":"content_block_stop","index":1}"#);
    accumulator.consume(r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#);
    accumulator.consume(r#"{"type":"message_stop"}"#);

    let stream = accumulator.finish().unwrap();
    assert_eq!(stream.end_turn, Some(false));
    assert!(crate::is_recoverable_tool_call_input(
        &stream.tool_calls[0].input
    ));
}

#[test]
fn openai_responses_captures_completed_server_state_reference() {
    let mut accumulator =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiResponses, Vec::new());
    accumulator
        .consume(r#"{"type":"response.completed","response":{"id":"resp-42","end_turn":true}}"#);

    assert_eq!(
        accumulator
            .finish()
            .unwrap()
            .server_conversation_state_ref
            .as_deref(),
        Some("resp-42")
    );
}

#[test]
fn non_responses_stream_does_not_capture_server_state_reference() {
    let mut accumulator =
        ProviderStreamAccumulator::new(ProviderStreamProtocol::OpenAiCompatible, Vec::new());
    accumulator.consume(r#"{"id":"resp-ignored","choices":[{"delta":{},"finish_reason":"stop"}]}"#);

    assert!(
        accumulator
            .finish()
            .unwrap()
            .server_conversation_state_ref
            .is_none()
    );
}
