use super::*;

const FINAL_TEXT: &str = "方案已落地并验证完成";
const WRITE_TOOL_NAME: &str = "builtin_write";

#[test]
fn protocol_stop_with_available_tools_completes_without_additional_model_request() {
    let mut input = test_transport_input();
    input.model_tools = vec![write_model_tool()];
    input.prompt_cache_plan = transport_prompt_cache_plan_for(
        ProviderStreamProtocol::OpenAiCompatible,
        input.model_tools.clone(),
    );
    let call_count = std::cell::Cell::new(0_u8);

    let report = AgentTurnRuntime
        .run(
            &input,
            |_request| {
                call_count.set(call_count.get() + 1);
                crate::test_stream_outcome(format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{FINAL_TEXT}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n"
                ))
            },
            |_transport_report, _provider_history, _turn_id| -> AxonResult<AgentTurnToolDecision<String>> {
                unreachable!("无工具调用时不应执行工具")
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(call_count.get(), 1);
    assert_eq!(report.content, FINAL_TEXT);
    assert_eq!(report.turn_reports.len(), 1);
    assert_eq!(
        report
            .turn_events
            .iter()
            .filter(|event| event.kind == AgentTurnEventKind::Completed)
            .count(),
        1
    );
}

#[test]
fn non_terminal_finish_reason_continues_until_protocol_stop() {
    let input = test_transport_input();
    let call_count = std::cell::Cell::new(0_u8);
    let second_request_body = std::cell::RefCell::new(String::new());

    let report = AgentTurnRuntime
        .run(
            &input,
            |request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt == 0 {
                    crate::test_stream_outcome(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"第一段\"},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n",
                    )
                } else {
                    second_request_body.replace(request.request.body_text.clone());
                    crate::test_stream_outcome(
                        "data: {\"choices\":[{\"delta\":{\"content\":\"第二段\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n",
                    )
                }
            },
            |_transport_report, _provider_history, _turn_id| -> AxonResult<AgentTurnToolDecision<String>> {
                unreachable!("无工具调用时不应执行工具")
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(call_count.get(), 2);
    assert_eq!(report.content, "第二段");
    assert!(second_request_body.borrow().contains("第一段"));
    assert_eq!(
        report
            .turn_events
            .iter()
            .filter(|event| event.kind == AgentTurnEventKind::Completed)
            .count(),
        1
    );
}

fn write_model_tool() -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: WRITE_TOOL_NAME.to_owned(),
        description: "写入文件".to_owned(),
        schema_hash: "write-tool-schema".to_owned(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
        input_format: axon_domain::ToolInputFormat::JsonSchema,
    }
}
