use std::path::PathBuf;

use axon_agent_runtime::ProviderStreamProtocol;
use axon_protocol::wire::WireEventKind;

use super::{RunProgressSink, RunProgressStreamNormalizer};

const TEST_PROVIDER_TURN_ID: u64 = 7;

#[test]
fn stream_normalizer_hides_split_text_tool_call_from_assistant_projection() {
    let mut normalizer = normalizer();
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        normalizer.consume_line(&openai_content_line("准备调用\n<function_"), &sink);
        normalizer.consume_line(
            &openai_content_line(
                "calls>\n<function name=\"builtin_read_file\">\n<args_json>{\"path\":\"docs/README.md\"}</args_json>\n</function>\n",
            ),
            &sink,
        );
        normalizer.consume_line(&openai_content_line("</function_calls>\n"), &sink);
        assert!(!normalizer.finish(&sink));
    }

    let visible = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::AssistantMessage(message) => Some(message.content.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(visible, "准备调用\n");
}

#[test]
fn stream_normalizer_flushes_unclosed_text_tool_call_as_visible_text() {
    let content = "<function_calls>\n<function name=\"builtin_read_file\">\n";
    let mut normalizer = normalizer();
    let mut events = Vec::new();

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());
        assert!(!normalizer.consume_line(&openai_content_line(content), &sink));
        assert!(normalizer.finish(&sink));
    }

    assert!(events.iter().any(|event| matches!(
        &event.event.kind,
        WireEventKind::AssistantMessage(message) if message.content == content
    )));
}

fn normalizer() -> RunProgressStreamNormalizer {
    RunProgressStreamNormalizer::new(
        ProviderStreamProtocol::OpenAiCompatible,
        TEST_PROVIDER_TURN_ID,
        PathBuf::from("."),
    )
}

fn openai_content_line(content: &str) -> String {
    format!(
        "data: {}",
        serde_json::json!({
            "choices": [{
                "delta": {"content": content},
                "finish_reason": null,
            }]
        })
    )
}
