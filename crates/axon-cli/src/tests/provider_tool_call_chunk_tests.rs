use axon_agent_runtime::{ModelToolDefinition, ProviderStreamProtocol, ProviderToolCallRuntime};

#[test]
fn openai_tool_call_arguments_are_reassembled_from_stream_chunks() {
    let report = ProviderToolCallRuntime
        .normalize_with_model_tools(
            ProviderStreamProtocol::OpenAiCompatible,
            openai_chunked_search_tool_call_body(),
            &[ModelToolDefinition {
                qualified_name: "builtin.search".to_owned(),
                description: "搜索 workspace 文件".to_owned(),
                schema_hash: "schema-hash".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
                input_format: axon_domain::ToolInputFormat::JsonSchema,
            }],
        )
        .expect("OpenAI-compatible 分片工具调用应可归一化");

    assert_eq!(report.tool_calls.len(), 1);
    let tool_call = &report.tool_calls[0];
    assert_eq!(tool_call.id, "call_chunked_search");
    assert_eq!(tool_call.name, "builtin.search");
    assert_eq!(
        tool_call.input.as_json(),
        Some(&serde_json::json!({
            "query": "workspace",
            "root": ".",
            "max_results": 1
        }))
    );
}

fn openai_chunked_search_tool_call_body() -> &'static str {
    "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_chunked_search\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"workspace\\\",\\\"root\\\":\\\".\\\",\\\"max_results\\\":\"}}]},\"finish_reason\":null}]}\n\n\
data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"1}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n\
data: [DONE]\n"
}
