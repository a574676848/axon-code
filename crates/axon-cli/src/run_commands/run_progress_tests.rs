use super::*;
use crate::tool_orchestrator::{ToolExecutionRequest, ToolKind};
use axon_agent_runtime::ProviderStreamProtocol;
use axon_agent_runtime::ProviderToolCall;
use axon_domain::{ExecutionRequestId, ToolCallId};
use std::path::PathBuf;

const TEST_PROVIDER_TURN_ID: u64 = 7;
const TEST_FREEFORM_PATCH: &str =
    "*** Begin Patch\n*** Add File: preview.rs\n+first\n+second\n*** End Patch";

#[test]
fn stream_normalizer_emits_openai_usage_update() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiCompatible,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"promptTokens":10000,"completionTokens":200,"totalTokens":10200,"promptTokensDetails":{"cachedTokens":9500,"cacheWriteTokens":200}}}"#,
            &sink,
        );
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::UsageUpdate(usage)
            if usage.input_tokens == 300
                && usage.cached_input_tokens == 9_500
                && usage.cache_write_input_tokens == 200
                && usage.current_context_tokens == 10_200
    )));
}

#[test]
fn stream_normalizer_emits_anthropic_usage_update() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::AnthropicMessages,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"type":"message_start","message":{"usage":{"input_tokens":11,"cache_creation_input_tokens":2,"cache_read_input_tokens":3,"output_tokens":0}}}"#,
            &sink,
        );
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::UsageUpdate(usage)
            if usage.input_tokens == 11
                && usage.cached_input_tokens == 3
                && usage.cache_write_input_tokens == 2
    )));
}

#[test]
fn stream_normalizer_emits_openai_reasoning_delta() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiCompatible,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"choices":[{"delta":{"reasoning_content":"需要先查上下文"},"finish_reason":null}]}"#,
            &sink,
        );
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningDelta(reasoning)
            if reasoning.part_id == "reasoning-part:7"
                && reasoning.reasoning_id == "provider-reasoning:7"
                && reasoning.delta == "需要先查上下文"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningStart(reasoning)
            if reasoning.part_id == "reasoning-part:7"
                && reasoning.reasoning_id == "provider-reasoning:7"
    )));
}

#[test]
fn stream_normalizer_emits_anthropic_reasoning_delta() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::AnthropicMessages,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先判断工具是否必要"}}"#,
            &sink,
        );
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningDelta(reasoning)
            if reasoning.part_id == "reasoning-part:7"
                && reasoning.reasoning_id == "provider-reasoning:7"
                && reasoning.delta == "先判断工具是否必要"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningStart(reasoning)
            if reasoning.part_id == "reasoning-part:7"
                && reasoning.reasoning_id == "provider-reasoning:7"
    )));
}

#[test]
fn finish_reasoning_sends_end_without_duplicate_delta_after_streaming() {
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        sink.reasoning_delta("reasoning:part:test", "provider-reasoning:test", "第一段");
        sink.finish_reasoning("reasoning:part:test", "provider-reasoning:test", "第一段");
    }

    assert!(!events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningDelta(reasoning)
            if reasoning.part_id == "reasoning:part:test"
                && reasoning.reasoning_id == "provider-reasoning:test"
                && reasoning.delta.is_empty()
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningEnd(reasoning)
            if reasoning.part_id == "reasoning:part:test"
                && reasoning.reasoning_id == "provider-reasoning:test"
    )));
}

#[test]
fn finish_reasoning_sends_full_delta_when_reasoning_was_not_streamed() {
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        sink.finish_reasoning(
            "reasoning:part:full",
            "provider-reasoning:full",
            "完整 reasoning",
        );
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningDelta(reasoning)
            if reasoning.part_id == "reasoning:part:full"
                && reasoning.reasoning_id == "provider-reasoning:full"
                && reasoning.delta == "完整 reasoning"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningEnd(reasoning)
            if reasoning.part_id == "reasoning:part:full"
                && reasoning.reasoning_id == "provider-reasoning:full"
    )));
}

#[test]
fn finish_reasoning_sends_missing_suffix_after_partial_streaming() {
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        sink.reasoning_delta("reasoning:part:test", "provider-reasoning:test", "第一段");
        sink.finish_reasoning(
            "reasoning:part:test",
            "provider-reasoning:test",
            "第一段第二段",
        );
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningDelta(reasoning)
            if reasoning.part_id == "reasoning:part:test"
                && reasoning.reasoning_id == "provider-reasoning:test"
                && reasoning.delta == "第二段"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::ReasoningEnd(reasoning)
            if reasoning.part_id == "reasoning:part:test"
                && reasoning.reasoning_id == "provider-reasoning:test"
    )));
}

#[test]
fn stream_normalizer_keeps_assistant_message_id_stable_across_text_deltas() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiCompatible,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"choices":[{"delta":{"content":"第一段"},"finish_reason":null}]}"#,
            &sink,
        );
        normalizer.consume_line(
            r#"data: {"choices":[{"delta":{"content":"第二段"},"finish_reason":null}]}"#,
            &sink,
        );
    }

    let assistant_messages = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::AssistantMessage(message) => Some((
                message.part_id.as_str(),
                message.message_id.as_str(),
                message.content.as_str(),
                event.event.sequence,
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        assistant_messages,
        vec![
            ("assistant-part:7", "provider-assistant:7", "第一段", 1),
            ("assistant-part:7", "provider-assistant:7", "第二段", 2),
        ]
    );
}

#[test]
fn stream_normalizer_emits_edit_only_at_stable_line_boundaries() {
    let workspace = axon_test_support::test_temp_dir();
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiCompatible,
        TEST_PROVIDER_TURN_ID,
        workspace.path().to_path_buf(),
    );
    let provider_line = |arguments: &str| {
        format!(
            "data: {}",
            serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-preview",
                            "function": {
                                "name": "builtin.edit",
                                "arguments": arguments
                            }
                        }]
                    }
                }]
            })
        )
    };
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        let mut generated = Vec::new();
        for arguments in [
            r#"{"input":"*** Begin Patch\n*** Add File: preview.rs\n+first\n+sec"#,
            r"ond\n",
            r#"*** End Patch"}"#,
        ] {
            generated.push(normalizer.consume_line(&provider_line(arguments), &sink));
        }
        assert!(generated.iter().all(|generated| !generated));
    }

    let changes = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::FileChange(change) => Some(change),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 2);
    assert!(matches!(
        &changes[0].change,
        axon_protocol::diff::FileChange::Add { content } if content == "first\n"
    ));
    assert!(matches!(
        &changes[1].change,
        axon_protocol::diff::FileChange::Add { content }
            if content == "first\nsecond\n"
    ));
    assert!(changes.iter().all(|change| {
        change.part_id == "diff:tool-part:stream:7:0:call-preview:preview.rs"
            && change.tool_part_id.as_deref() == Some("tool-part:stream:7:0:call-preview")
    }));
}

#[test]
fn stream_normalizer_emits_edit_patch_sections_without_duplicate_final_snapshot() {
    let workspace = axon_test_support::test_temp_dir();
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiCompatible,
        TEST_PROVIDER_TURN_ID,
        workspace.path().to_path_buf(),
    );
    let provider_line = |arguments: &str| {
        format!(
            "data: {}",
            serde_json::json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-patch-preview",
                            "function": {
                                "name": "builtin.edit",
                                "arguments": arguments
                            }
                        }]
                    }
                }]
            })
        )
    };
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        for arguments in [
            r#"{"input":"*** Begin Patch\n*** Add File: preview.rs\n+first\n"#,
            r"+second\n",
            r#"*** End Patch"}"#,
        ] {
            normalizer.consume_line(&provider_line(arguments), &sink);
        }
    }

    let changes = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::FileChange(change) => Some(change),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 2);
    assert!(matches!(
        &changes[0].change,
        axon_protocol::diff::FileChange::Add { content } if content == "first\n"
    ));
    assert!(matches!(
        &changes[1].change,
        axon_protocol::diff::FileChange::Add { content }
            if content == "first\nsecond\n"
    ));
    assert!(changes.iter().all(|change| {
        change.part_id == "diff:tool-part:stream:7:0:call-patch-preview:preview.rs"
            && change.tool_part_id.as_deref() == Some("tool-part:stream:7:0:call-patch-preview")
    }));
}

#[test]
fn responses_custom_patch_stream_emits_incremental_diff_without_duplicate_final_snapshot() {
    let workspace = axon_test_support::test_temp_dir();
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiResponses,
        TEST_PROVIDER_TURN_ID,
        workspace.path().to_path_buf(),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        let lines = [
            serde_json::json!({
                "type": "response.output_item.added",
                "output_index": 0,
                "item": {
                    "type": "custom_tool_call",
                    "call_id": "call-custom-patch",
                    "name": "builtin.edit"
                }
            }),
            serde_json::json!({
                "type": "response.custom_tool_call_input.delta",
                "output_index": 0,
                "delta": "*** Begin Patch\n*** Add File: preview.rs\n+first\n"
            }),
            serde_json::json!({
                "type": "response.custom_tool_call_input.delta",
                "output_index": 0,
                "delta": "+second\n"
            }),
            serde_json::json!({
                "type": "response.custom_tool_call_input.done",
                "output_index": 0,
                "input": TEST_FREEFORM_PATCH
            }),
        ];
        for value in lines {
            normalizer.consume_line(&format!("data: {value}"), &sink);
        }
    }

    let changes = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::FileChange(change) => Some(change),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 2);
    assert!(matches!(
        &changes[0].change,
        axon_protocol::diff::FileChange::Add { content } if content == "first\n"
    ));
    assert!(matches!(
        &changes[1].change,
        axon_protocol::diff::FileChange::Add { content }
            if content == "first\nsecond\n"
    ));
}

#[test]
fn edit_patch_start_emits_diff_snapshot() {
    let mut events = Vec::new();
    let request = ToolExecutionRequest {
        tool_call: ProviderToolCall::new(
            "call-1",
            "builtin.edit",
            serde_json::json!({
                "input": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch"
            })
            .to_string(),
        ),
        tool_call_id: ToolCallId(1),
        part_id: "tool-part:test:progress:1".to_owned(),
        execution_request_id: ExecutionRequestId(1),
        kind: ToolKind::Edit,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        sink.tool_execution_started(&request);
    }

    assert!(matches!(
        &events[0].event.kind,
        WireEventKind::ToolCall(call) if call.tool_call_id == "call-1"
    ));
    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::FileChange(change) if change.path == "src/lib.rs"
    )));
}

#[test]
fn repeated_usage_frames_of_one_stream_are_emitted_once() {
    let mut events = Vec::new();
    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        sink.begin_attempt_composition(Some(ContextTokenComposition {
            system_prompt_tokens: 17_637,
            tool_definition_tokens: 41_897,
            conversation_tokens: 2_529,
        }));
        let usage =
            TokenUsageTelemetry::new(3_844, 59_789, 0, 0, 156).with_current_context_tokens(63_836);

        // 流内实时帧与收尾补发的最终用量是同一份快照：只投一次。
        sink.token_usage_updated(usage);
        sink.token_usage_updated(usage);
    }

    let usage_events: Vec<_> = events
        .iter()
        .filter(|event| matches!(event.event.kind, WireEventKind::UsageUpdate(_)))
        .collect();
    assert_eq!(usage_events.len(), 1, "同一条流的同一份快照不得重复投递");
    let WireEventKind::UsageUpdate(usage) = &usage_events[0].event.kind else {
        panic!("事件类型应为用量更新");
    };
    // 构成随实时帧一起投出：收尾补发被吸收之后，界面仍然拿得到构成。
    assert_eq!(usage.system_prompt_tokens, 17_637);
    assert_eq!(usage.tool_definition_tokens, 41_897);
    assert_eq!(usage.conversation_tokens, 2_529);
}

#[test]
fn changed_snapshots_and_new_streams_are_still_emitted() {
    let mut events = Vec::new();
    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        let first = TokenUsageTelemetry::new(100, 0, 0, 0, 40);
        let grown = TokenUsageTelemetry::new(100, 0, 0, 0, 90);

        sink.token_usage_updated(first);
        // 流内累计增长：数值变了就是新观测，必须投出。
        sink.token_usage_updated(grown);
        // 换一条 provider 流：即使数值与上一条流完全相同，也要各自投一次。
        sink.streaming_started("cpa", "gemini-3.8-flash", 1);
        sink.token_usage_updated(grown);
    }

    let usage_events: Vec<_> = events
        .iter()
        .filter(|event| matches!(event.event.kind, WireEventKind::UsageUpdate(_)))
        .collect();
    assert_eq!(usage_events.len(), 3, "增长的快照与新流都要投出");
}
