use super::*;
use std::path::PathBuf;

use axon_agent_runtime::ProviderStreamProtocol;
use axon_protocol::wire::WireEventKind;

const TEST_PROVIDER_TURN_ID: u64 = 7;

#[test]
fn stream_normalizer_emits_reasoning_end_at_openai_item_boundary() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiResponses,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"已完成分析"}"#,
            &sink,
        );
        normalizer.consume_line(
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1"}}"#,
            &sink,
        );
    }

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event.kind, WireEventKind::ReasoningEnd(_)))
            .count(),
        1
    );
}

#[test]
fn stream_normalizer_keeps_openai_reasoning_items_independent() {
    let mut normalizer = RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiResponses,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    );
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_1","delta":"第一段"}"#,
            &sink,
        );
        normalizer.consume_line(
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_1"}}"#,
            &sink,
        );
        normalizer.consume_line(
            r#"data: {"type":"response.reasoning_summary_text.delta","item_id":"rs_2","delta":"第二段"}"#,
            &sink,
        );
        normalizer.consume_line(
            r#"data: {"type":"response.output_item.done","item":{"type":"reasoning","id":"rs_2"}}"#,
            &sink,
        );
    }

    let parts = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::ReasoningStart(reasoning) => Some(reasoning.part_id.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(parts, vec!["reasoning-part:7", "reasoning-part:7:1"]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event.kind, WireEventKind::ReasoningEnd(_)))
            .count(),
        2
    );
}

#[test]
fn stream_normalizer_emits_reasoning_end_at_anthropic_block_boundary() {
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
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"已完成分析"}}"#,
            &sink,
        );
        normalizer.consume_line(r#"data: {"type":"content_block_stop","index":0}"#, &sink);
    }

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event.kind, WireEventKind::ReasoningEnd(_)))
            .count(),
        1
    );
}

#[test]
fn explicit_reasoning_end_is_not_repeated_during_settlement() {
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        sink.reasoning_delta("reasoning:part:test", "provider-reasoning:test", "第一段");
        sink.end_reasoning("reasoning:part:test", "provider-reasoning:test");
        sink.finish_reasoning("reasoning:part:test", "provider-reasoning:test", "第一段");
    }

    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event.kind, WireEventKind::ReasoningEnd(_)))
            .count(),
        1
    );
}
