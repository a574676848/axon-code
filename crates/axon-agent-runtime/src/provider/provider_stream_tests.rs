use super::*;

use crate::provider_stream_error::PROVIDER_STREAM_ERROR_EVENT_PREFIX;

#[test]
fn openai_stream_rejects_top_level_error_event() {
    let result = OpenAiCompatibleStreamRuntime.normalize_text_deltas(
        crate::ProviderStreamProtocol::OpenAiCompatible,
        r#"data: {"error":{"message":"rate limited","type":"rate_limit_error","code":"rate_limit_exceeded"}}
data: [DONE]
"#,
    );

    assert_provider_error_event(
        result,
        "rate limited",
        "rate_limit_error",
        "rate_limit_exceeded",
    );
}

#[test]
fn openai_responses_stream_accepts_text_events_without_choices() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.created","response":{"id":"resp_text"}}
data: {"type":"response.output_text.delta","delta":"Axon Feishu live provider ok"}
data: {"type":"response.completed","response":{"id":"resp_text"}}
"#,
        )
        .expect("OpenAI Responses 文本流应可归一化");

    assert_eq!(result.deltas.len(), 1);
    assert_eq!(
        result.deltas[0].content_delta,
        "Axon Feishu live provider ok"
    );
    assert_eq!(result.finish_reason.as_deref(), Some("stop"));
}

#[test]
fn openai_responses_stream_projects_reasoning_summary_text_delta() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_1","summary_index":0,"delta":"先确认调用链"}
data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"不应重复回填"}]}}
data: {"type":"response.completed","response":{"id":"resp_reasoning"}}
"#,
        )
        .expect("OpenAI Responses reasoning summary delta 应可归一化");

    assert_eq!(result.reasoning_content.as_deref(), Some("先确认调用链"));
}

#[test]
fn openai_responses_stream_does_not_backfill_final_reasoning_summary() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1","summary":[{"type":"summary_text","text":"仅最终事件携带的摘要"}]}}
data: {"type":"response.completed","response":{"id":"resp_reasoning"}}
"#,
        )
        .expect("仅最终 summary 的 OpenAI Responses 流仍应正常完成");

    assert!(result.reasoning_content.is_none());
}

#[test]
fn openai_responses_stream_projects_completed_usage() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.output_text.delta","delta":"pong"}
data: {"type":"response.completed","response":{"id":"resp_text","usage":{"input_tokens":11,"input_tokens_details":{"cached_tokens":3},"output_tokens":7,"output_tokens_details":{"reasoning_tokens":2},"total_tokens":18}}}
"#,
        )
        .expect("OpenAI Responses usage 应从 completed 事件归一化");

    let usage = result.token_usage.expect("completed event usage");
    assert_eq!(usage.input_tokens, 8);
    assert_eq!(usage.cache_read_input_tokens, 3);
    assert_eq!(usage.cache_write_input_tokens, 0);
    assert_eq!(usage.complete_input_tokens(), 11);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.reasoning_output_tokens, 2);
    assert_eq!(usage.current_context_tokens, 18);
}

#[test]
fn openai_responses_stream_projects_camel_case_usage() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.completed","response":{"id":"resp_text","usage":{"inputTokens":21,"inputTokensDetails":{"cachedTokens":5},"outputTokens":8,"outputTokensDetails":{"reasoningTokens":3}}}}
"#,
        )
        .expect("OpenAI Responses camelCase usage 应可归一化");

    let usage = result.token_usage.expect("completed event usage");
    assert_eq!(usage.input_tokens, 16);
    assert_eq!(usage.cache_read_input_tokens, 5);
    assert_eq!(usage.cache_write_input_tokens, 0);
    assert_eq!(usage.complete_input_tokens(), 21);
    assert_eq!(usage.output_tokens, 5);
    assert_eq!(usage.reasoning_output_tokens, 3);
}

#[test]
fn openai_responses_stream_projects_top_level_completed_usage() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiResponses,
            r#"data: {"type":"response.completed","usage":{"input_tokens":13,"input_tokens_details":{"cached_tokens":4},"output_tokens":9,"output_tokens_details":{"reasoning_tokens":3}}}"#,
        )
        .expect("OpenAI Responses 顶层 usage 应可归一化");

    let usage = result.token_usage.expect("top-level completed usage");
    assert_eq!(usage.input_tokens, 9);
    assert_eq!(usage.cache_read_input_tokens, 4);
    assert_eq!(usage.cache_write_input_tokens, 0);
    assert_eq!(usage.complete_input_tokens(), 13);
    assert_eq!(usage.output_tokens, 6);
    assert_eq!(usage.reasoning_output_tokens, 3);
}

#[test]
fn openai_compatible_stream_projects_total_only_usage() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"content":"pong"},"finish_reason":null}]}
data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"totalTokens":34}}
data: [DONE]"#,
        )
        .expect("OpenAI-compatible total-only usage 应保留非零观测");

    let usage = result.token_usage.expect("stream usage");
    assert_eq!(usage.total_tokens(), 34);
    assert_eq!(usage.current_context_tokens(), 34);
}

#[test]
fn openai_partial_stream_projects_usage_frame() {
    let usage = OpenAiCompatibleStreamRuntime
        .normalize_partial_token_usage(
            crate::ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"promptTokens":10000,"completionTokens":200,"totalTokens":10200,"promptTokensDetails":{"cachedTokens":9500,"cacheWriteTokens":200}}}"#,
        )
        .expect("partial usage 应可实时归一化");

    assert_eq!(usage.input_tokens, 300);
    assert_eq!(usage.cache_read_input_tokens, 9_500);
    assert_eq!(usage.cache_write_input_tokens, 200);
    assert_eq!(usage.complete_input_tokens(), 10_000);
}

#[test]
fn openai_compatible_stream_preserves_high_cache_read_and_write() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"content":"pong"},"finish_reason":null}]}
data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10000,"completion_tokens":200,"total_tokens":10200,"prompt_tokens_details":{"cached_tokens":9500,"cache_write_tokens":200}}}
data: [DONE]"#,
        )
        .expect("OpenAI-compatible 高缓存 usage 应可归一化");

    let usage = result.token_usage.expect("high cache usage");
    assert_eq!(usage.input_tokens, 300);
    assert_eq!(usage.cache_read_input_tokens, 9_500);
    assert_eq!(usage.cache_write_input_tokens, 200);
    assert_eq!(usage.complete_input_tokens(), 10_000);
    assert_eq!(usage.output_tokens, 200);
    assert_eq!(usage.total_tokens(), 10_200);
    assert_eq!(usage.current_context_tokens(), 10_200);
}

#[test]
fn anthropic_stream_preserves_cache_write_from_message_start() {
    let result = AnthropicMessagesStreamRuntime
        .normalize_text_deltas(
            r#"event: message_start
data: {"type":"message_start","message":{"usage":{"input_tokens":11,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":0}}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}

event: message_stop
data: {"type":"message_stop"}"#,
        )
        .expect("Anthropic cache write 应跨 message_delta 保留");

    let usage = result.token_usage.expect("anthropic usage");
    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.cache_read_input_tokens, 3);
    assert_eq!(usage.cache_write_input_tokens, 2);
    assert_eq!(usage.complete_input_tokens(), 16);
    assert_eq!(usage.output_tokens, 7);
    assert_eq!(usage.current_context_tokens, 23);
}

#[test]
fn openai_responses_partial_stream_projects_reasoning_delta() {
    let report = OpenAiCompatibleStreamRuntime
        .normalize_partial_text_deltas(
            r#"data: {"type":"response.reasoning_text.delta","delta":"先确认调用链"}
"#,
            0,
        )
        .expect("OpenAI Responses partial reasoning 应可归一化");

    assert!(report.deltas.is_empty());
    assert_eq!(report.reasoning_deltas.len(), 1);
    assert_eq!(report.reasoning_deltas[0].content_delta, "先确认调用链");
}

#[test]
fn openai_responses_partial_stream_projects_reasoning_summary_text_delta() {
    let report = OpenAiCompatibleStreamRuntime
        .normalize_partial_text_deltas(
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_1","summary_index":0,"delta":"先检查实时链路"}
"#,
            0,
        )
        .expect("OpenAI Responses partial reasoning summary 应可归一化");

    assert!(report.deltas.is_empty());
    assert_eq!(report.reasoning_deltas.len(), 1);
    assert_eq!(report.reasoning_deltas[0].content_delta, "先检查实时链路");
}

#[test]
fn openai_responses_partial_stream_marks_reasoning_item_done() {
    let report = OpenAiCompatibleStreamRuntime
        .normalize_partial_text_deltas(
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1"}}
"#,
            0,
        )
        .expect("OpenAI Responses reasoning 结束帧应可归一化");

    assert!(report.reasoning_deltas.is_empty());
    assert!(report.reasoning_end);
    assert_eq!(report.reasoning_event_key.as_deref(), Some("rs_1"));
}

#[test]
fn openai_compatible_partial_stream_projects_reasoning_aliases_without_duplication() {
    let report = OpenAiCompatibleStreamRuntime
        .normalize_partial_text_deltas(
            r#"data: {"choices":[{"delta":{"reasoning":"主字段"}}]}
data: {"choices":[{"delta":{"reasoning_content":"兼容字段"}}]}
data: {"choices":[{"delta":{"reasoning":"优先字段","reasoning_content":"不应重复"}}]}
data: {"choices":[{"delta":{"reasoning":"","reasoning_content":"空主字段回退"}}]}
"#,
            0,
        )
        .expect("OpenAI-compatible partial reasoning 字段应统一归一化");

    assert!(report.deltas.is_empty());
    assert_eq!(
        report
            .reasoning_deltas
            .iter()
            .map(|delta| delta.content_delta.as_str())
            .collect::<Vec<_>>(),
        ["主字段", "兼容字段", "优先字段", "空主字段回退"]
    );
}

#[test]
fn anthropic_partial_stream_projects_reasoning_delta() {
    let report = AnthropicMessagesStreamRuntime
        .normalize_partial_text_deltas(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先判断是否需要工具"}}
"#,
            0,
        )
        .expect("Anthropic partial thinking_delta 应投射为 reasoning delta");

    assert!(report.deltas.is_empty());
    assert_eq!(report.reasoning_deltas.len(), 1);
    assert_eq!(
        report.reasoning_deltas[0].content_delta,
        "先判断是否需要工具"
    );
}

#[test]
fn anthropic_partial_stream_marks_content_block_stop_as_reasoning_end() {
    let report = AnthropicMessagesStreamRuntime
        .normalize_partial_text_deltas(
            r#"data: {"type":"content_block_stop","index":0}
"#,
            0,
        )
        .expect("Anthropic thinking block 结束帧应可归一化");

    assert!(report.reasoning_deltas.is_empty());
    assert!(report.reasoning_end);
    assert_eq!(report.reasoning_event_key.as_deref(), Some("0"));
}

#[test]
fn anthropic_partial_stream_projects_usage_frame() {
    let usage = AnthropicMessagesStreamRuntime
        .normalize_partial_token_usage(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":11,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":0}}}"#,
        )
        .expect("Anthropic partial usage 应可实时归一化");

    assert_eq!(usage.input_tokens, 11);
    assert_eq!(usage.cache_read_input_tokens, 3);
    assert_eq!(usage.cache_write_input_tokens, 2);
}

#[test]
fn openai_compatible_stream_projects_camel_case_usage() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"content":"pong"},"finish_reason":null}]}
data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"promptTokens":30,"completionTokens":12,"totalTokens":42,"promptTokensDetails":{"cachedTokens":4},"completionTokensDetails":{"reasoningTokens":2}}}
data: [DONE]"#,
        )
        .expect("OpenAI-compatible camelCase usage 应可归一化");

    let usage = result.token_usage.expect("stream usage");
    assert_eq!(usage.input_tokens, 26);
    assert_eq!(usage.cache_read_input_tokens, 4);
    assert_eq!(usage.cache_write_input_tokens, 0);
    assert_eq!(usage.output_tokens, 10);
    assert_eq!(usage.reasoning_output_tokens, 2);
}

#[test]
fn openai_compatible_stream_accepts_tool_only_completed_turn() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"role":"assistant"},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_search","type":"function","function":{"name":"builtin_search","arguments":"{\"query\":"}}]},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"provider_stream\"}"}}]},"finish_reason":null}]}

data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]
"#,
        )
        .expect("OpenAI-compatible 纯工具调用完成流应可归一化");

    assert!(result.deltas.is_empty());
    assert!(result.done_received);
    assert_eq!(result.finish_reason.as_deref(), Some("tool_calls"));
}

#[test]
fn anthropic_messages_stream_accepts_tool_only_completed_turn() {
    let result = AnthropicMessagesStreamRuntime
        .normalize_text_deltas(
            r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"need tool"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"builtin_search","input":{}}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"provider_stream\"}"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}

data: {"type":"message_stop"}
"#,
        )
        .expect("Anthropic 纯工具调用完成流应可归一化");

    assert!(result.deltas.is_empty());
    assert!(result.message_stop_received);
    assert_eq!(result.stop_reason.as_deref(), Some("tool_use"));
}

#[test]
fn anthropic_messages_stream_accepts_normalized_event_data() {
    let result = AnthropicMessagesStreamRuntime
        .normalize_text_deltas(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"pong"}}

data: {"type":"message_stop"}

"#,
        )
        .expect("Anthropic Messages 规整事件流应可归一化");

    assert_eq!(result.deltas.len(), 1);
    assert_eq!(result.deltas[0].content_delta, "pong");
    assert!(result.message_stop_received);
}

#[test]
fn openai_compatible_stream_accepts_standalone_usage_frame() {
    let result = OpenAiCompatibleStreamRuntime
        .normalize_text_deltas(
            crate::ProviderStreamProtocol::OpenAiCompatible,
            r#"data: {"choices":[{"delta":{"content":"pong"},"finish_reason":null}]}
data: {"choices":[{"delta":{},"finish_reason":"stop"}]}
data: {"usage":{"prompt_tokens":10000,"completion_tokens":200,"total_tokens":10200,"prompt_tokens_details":{"cached_tokens":9500}}}
data: [DONE]"#,
        )
        .expect("独立 usage 帧不应触发 choices 缺失错误");

    let usage = result.token_usage.expect("standalone usage frame");
    assert_eq!(usage.input_tokens, 500);
    assert_eq!(usage.cache_read_input_tokens, 9_500);
    assert_eq!(usage.complete_input_tokens(), 10_000);
    assert_eq!(usage.output_tokens, 200);
    assert_eq!(usage.current_context_tokens(), 10_200);
}

#[test]
fn partial_normalizer_advances_text_sequence_only_after_projection_commit() {
    let mut normalizer =
        ProviderStreamPartialNormalizer::new(crate::ProviderStreamProtocol::OpenAiCompatible);
    let first = normalizer
        .normalize(r#"data: {"choices":[{"delta":{"content":"first"}}]}"#)
        .expect("first delta");
    assert_eq!(first.deltas[0].sequence, 0);

    let retry = normalizer
        .normalize(r#"data: {"choices":[{"delta":{"content":"retry"}}]}"#)
        .expect("uncommitted delta");
    assert_eq!(retry.deltas[0].sequence, 0);

    normalizer.commit_text_deltas(first.deltas.len());
    let next = normalizer
        .normalize(r#"data: {"choices":[{"delta":{"content":"next"}}]}"#)
        .expect("committed delta");
    assert_eq!(next.deltas[0].sequence, 1);
}

fn assert_provider_error_event<T>(
    result: AxonResult<T>,
    expected_message: &str,
    expected_type: &str,
    expected_code: &str,
) {
    let Err(AxonError::Provider(axon_domain::ProviderError::Transport {
        message,
        http_status,
        ..
    })) = result
    else {
        panic!("expected structured provider stream error event");
    };
    assert_eq!(http_status, None);
    assert!(message.starts_with(PROVIDER_STREAM_ERROR_EVENT_PREFIX));
    assert!(message.contains(expected_message));
    assert!(message.contains(expected_type));
    assert!(message.contains(expected_code));
}
