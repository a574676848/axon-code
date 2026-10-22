use crate::prompt_cache_tool_wire;
use crate::token_estimator::estimated_message_tokens;
use crate::{
    AgentModelLocalFileRef, AgentModelLocalImageRef, AgentModelMessage, AgentModelToolCallMetadata,
    ModelToolDefinition, PromptCacheProviderWireFormat, PromptCacheProviderWireInput,
    PromptCacheProviderWirePlan, PromptCacheProviderWireRuntime, ProviderPromptCachePlan,
    ProviderPromptCachePlanInput, ProviderPromptCachePlanRuntime,
};

const FIRST_FILE_PREVIEW: &str = "[文件预览] path=C:\\work\\first.txt name=first.txt mime=text/plain bytes=11 archiveRef=archive:first sourceRef=upload:first\nfirst body";
const SECOND_FILE_PREVIEW: &str = "[文件预览] path=C:\\work\\second.txt name=second.txt mime=text/plain bytes=12 archiveRef=archive:second sourceRef=upload:second\nsecond body";

#[test]
fn anthropic_multi_file_message_marks_only_last_preview() {
    let message = file_message();
    let blocks = prompt_cache_tool_wire::anthropic_content_blocks(&message, true, &[]);

    assert_eq!(cache_control_count(&blocks), 1);
    assert!(blocks[0].get("cache_control").is_none());
    assert!(blocks[1].get("cache_control").is_none());
    assert_eq!(blocks[2]["text"], SECOND_FILE_PREVIEW);
    assert_eq!(
        blocks[2]["cache_control"]["type"],
        crate::PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
}

#[test]
fn anthropic_file_anchor_obeys_global_breakpoint_cap() {
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: anthropic_capabilities(),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("system", "system prompt"),
            AgentModelMessage::text("user", "earlier request"),
            AgentModelMessage::text("assistant", "earlier response"),
            file_message(),
        ]
        .into(),
        model_tools: vec![model_tool()],
        prompt_cache_plan: test_prompt_cache_plan(),
    });

    assert_eq!(anthropic_cache_control_count(&plan), 4);
    let latest_content = plan.anthropic_messages[2]["content"]
        .as_array()
        .expect("latest Anthropic message content");
    assert_eq!(cache_control_count(latest_content), 1);
    assert_eq!(
        latest_content[2]["cache_control"]["type"],
        crate::PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
}

#[test]
fn anthropic_message_without_files_keeps_primary_text_anchor() {
    let message = AgentModelMessage::text("user", "plain request");
    let blocks = prompt_cache_tool_wire::anthropic_content_blocks(&message, true, &[]);

    assert_eq!(cache_control_count(&blocks), 1);
    assert_eq!(
        blocks[0]["cache_control"]["type"],
        crate::PROMPT_CACHE_CONTROL_TYPE_EPHEMERAL
    );
}

#[test]
fn anthropic_tail_backfills_past_image_only_and_tool_only_messages() {
    let image_only = AgentModelMessage::text("user", "").with_local_images(vec![
        AgentModelLocalImageRef::local("image.png", "image/png"),
    ]);
    let tool_only = AgentModelMessage::text("assistant", "").with_tool_calls(vec![
        AgentModelToolCallMetadata {
            id: "call-1".to_owned(),
            name: "builtin.search".to_owned(),
            input: axon_domain::ToolCallInput::json(serde_json::json!({"query": "axon"})),
        },
    ]);
    let plan = PromptCacheProviderWireRuntime.lower(&PromptCacheProviderWireInput {
        format: PromptCacheProviderWireFormat::AnthropicMessages,
        capabilities: anthropic_capabilities(),
        supports_vision: true,
        messages: vec![
            AgentModelMessage::text("user", "older request"),
            AgentModelMessage::text("assistant", "older response"),
            image_only,
            tool_only,
            AgentModelMessage::tool_result("call-1", "matched", true),
        ]
        .into(),
        model_tools: Vec::new(),
        prompt_cache_plan: test_prompt_cache_plan(),
    });

    let marked_text = plan
        .anthropic_messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter(|block| block.get("cache_control").is_some())
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(marked_text, ["older request", "older response"]);
    assert_eq!(anthropic_cache_control_count(&plan), 2);
    assert!(plan.anthropic_messages.iter().all(|message| {
        message["content"]
            .as_array()
            .expect("Anthropic content blocks")
            .iter()
            .filter(|block| matches!(block["type"].as_str(), Some("image" | "tool_use")))
            .all(|block| block.get("cache_control").is_none())
    }));
}

#[test]
fn three_protocol_file_projection_matches_estimator_visible_content() {
    let message = file_message();
    let text_only = AgentModelMessage::text("user", "inspect files");

    for format in provider_formats() {
        let items = prompt_cache_tool_wire::model_visible_items(&message, format, &[]);
        let previews = projected_file_previews(&items, format);
        assert_eq!(previews, [FIRST_FILE_PREVIEW, SECOND_FILE_PREVIEW]);
        assert!(
            estimated_message_tokens(&message, format, &[])
                > estimated_message_tokens(&text_only, format, &[]),
            "format={format:?}"
        );
    }
}

fn file_message() -> AgentModelMessage {
    AgentModelMessage::text("user", "inspect files").with_local_files(vec![
        local_file("first", "first body", 11),
        local_file("second", "second body", 12),
    ])
}

fn local_file(id: &str, preview: &str, byte_len: u64) -> AgentModelLocalFileRef {
    AgentModelLocalFileRef {
        name: format!("{id}.txt"),
        path: Some(format!("C:\\work\\{id}.txt")),
        mime_type: "text/plain".to_owned(),
        preview: preview.to_owned(),
        byte_len: Some(byte_len),
        sha256: Some(format!("sha256:{id}")),
        archive_ref: Some(format!("archive:{id}")),
        source_ref: Some(format!("upload:{id}")),
    }
}

fn provider_formats() -> [PromptCacheProviderWireFormat; 3] {
    [
        PromptCacheProviderWireFormat::OpenAiCompatibleChat,
        PromptCacheProviderWireFormat::OpenAiResponses,
        PromptCacheProviderWireFormat::AnthropicMessages,
    ]
}

fn projected_file_previews(
    items: &[serde_json::Value],
    format: PromptCacheProviderWireFormat,
) -> [&str; 2] {
    let content = match format {
        PromptCacheProviderWireFormat::OpenAiCompatibleChat => &items[0]["content"],
        PromptCacheProviderWireFormat::OpenAiResponses => &items[0]["content"],
        PromptCacheProviderWireFormat::AnthropicMessages => &items[0]["content"],
    };
    let parts = content.as_array().expect("provider visible content parts");
    [
        parts[1]["text"].as_str().expect("first file preview"),
        parts[2]["text"].as_str().expect("second file preview"),
    ]
}

fn anthropic_capabilities() -> axon_config::ProviderCapabilities {
    axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_ANTHROPIC)
}

fn test_prompt_cache_plan() -> ProviderPromptCachePlan {
    ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: anthropic_capabilities(),
            format: PromptCacheProviderWireFormat::AnthropicMessages,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: true,
            messages: Vec::new().into(),
            model_tools: Vec::new(),
            context_section_count: 0,
        })
        .expect("test prompt cache plan")
}

fn model_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: "builtin.search".to_owned(),
        description: "search workspace".to_owned(),
        schema_hash: "tool-schema-search".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {"query": {"type": "string"}},
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}

fn cache_control_count(blocks: &[serde_json::Value]) -> usize {
    blocks
        .iter()
        .filter(|block| block.get("cache_control").is_some())
        .count()
}

fn anthropic_cache_control_count(plan: &PromptCacheProviderWirePlan) -> usize {
    let system = cache_control_count(&plan.anthropic_system);
    let messages = plan
        .anthropic_messages
        .iter()
        .filter_map(|message| message.get("content").and_then(serde_json::Value::as_array))
        .map(|blocks| cache_control_count(blocks))
        .sum::<usize>();
    let tools = cache_control_count(&plan.anthropic_tools);
    system + messages + tools
}
