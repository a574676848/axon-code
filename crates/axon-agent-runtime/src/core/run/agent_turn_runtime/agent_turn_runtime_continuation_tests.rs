use super::*;
use axon_client_identity::ClientChannel;

#[path = "agent_turn_runtime_end_state_tests.rs"]
mod agent_turn_runtime_end_state_tests;

#[test]
fn deferred_diagnostics_runtime_note_reaches_next_provider_turn() {
    let input = test_transport_input();
    let source = TestSteerSource::from_message(AgentModelMessage::runtime_note(
        AgentRuntimeNoteKind::DeferredDiagnostics,
        "late diagnostics",
    ));
    let request_bodies = std::cell::RefCell::new(Vec::new());

    let report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                request_bodies
                    .borrow_mut()
                    .push(transport_request.request.body_text.clone());
                crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
            },
            |_transport_report, _provider_history, _turn_id| {
                Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                    AgentTurnToolDecision::AwaitExternal {
                        receipts: Vec::new(),
                    },
                )
            },
            Some(&source),
            None,
        )
        .unwrap();

    assert_eq!(report.turn_reports.len(), 2);
    let second_body: serde_json::Value = serde_json::from_str(&request_bodies.borrow()[1]).unwrap();
    let messages = second_body["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"].as_str().is_some_and(|content| {
                content.contains("<runtime-note kind=\"deferred-diagnostics\">")
                    && content.contains("late diagnostics")
            })
    }));
}

#[test]
fn completion_notice_runtime_note_reaches_next_provider_turn() {
    let input = test_transport_input();
    // 与 DeferredDiagnostics 同构：不同注入类型走同一条 steer 通道，
    // 但 tag 必须是独立的 `completion-notice`，消费侧才分得清「诊断」与「作业完成」。
    let source = TestSteerSource::from_message(AgentModelMessage::runtime_note(
        AgentRuntimeNoteKind::CompletionNotice,
        "临时 Job 完成通知（生产者为 subagent）：\n- 作业引用：epoch:7\n- 终态：completed",
    ));
    let request_bodies = std::cell::RefCell::new(Vec::new());

    let report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                request_bodies
                    .borrow_mut()
                    .push(transport_request.request.body_text.clone());
                crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
            },
            |_transport_report, _provider_history, _turn_id| {
                Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                    AgentTurnToolDecision::AwaitExternal {
                        receipts: Vec::new(),
                    },
                )
            },
            Some(&source),
            None,
        )
        .unwrap();

    assert_eq!(
        report.turn_reports.len(),
        2,
        "完成通知必须触发下一轮 provider 请求"
    );
    let second_body: serde_json::Value = serde_json::from_str(&request_bodies.borrow()[1]).unwrap();
    let messages = second_body["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"].as_str().is_some_and(|content| {
                content.contains("<runtime-note kind=\"completion-notice\">")
                    && content.contains("临时 Job 完成通知")
            })
    }));
}

#[test]
fn agent_turn_runtime_keeps_continuing_until_completion() {
    let input = ProviderStreamingTransportInput {
        client_channel: ClientChannel::Cli,
        turn_id: axon_domain::TurnId(1),
        provider_name: "test-provider".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        base_url: "https://provider.example/v1".to_owned(),
        api_key_env: None,
        api_key: None,
        model_id: "test-model".to_owned(),
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
        structured_output: None,
        context_window: 32_000,
        supports_tools: true,
        supports_vision: false,
        messages: vec![AgentModelMessage::text("user", "ping")].into(),
        model_tools: Vec::new(),
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan: transport_prompt_cache_plan(),
        anthropic_max_tokens: 4096,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
    };
    let call_count = std::cell::Cell::new(0_u8);
    let report = AgentTurnRuntime
        .run(
            &input,
            |_transport_request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt < 4 {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_search\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"ping\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n")
                } else {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
                }
            },
            |transport_report, _provider_history, turn_id| {
                if transport_report.runner_report.tool_calls.is_empty() {
                    Ok(AgentTurnToolDecision::AwaitExternal {
                        receipts: Vec::<String>::new(),
                    })
                } else {
                    Ok(AgentTurnToolDecision::Continue {
                        receipts: vec![format!("turn-{}", turn_id.0)],
                        next_messages: vec![
                            AgentModelMessage::text("assistant", "").with_tool_calls(vec![
                                crate::AgentModelToolCallMetadata {
                                    id: "call_search".to_owned(),
                                    name: "builtin.search".to_owned(),
                                    input: "{}".into(),
                                },
                            ]),
                            AgentModelMessage::tool_result(
                                "call_search",
                                format!("status=completed;turn={}", turn_id.0),
                                true,
                            ),
                        ],
                    })
                }
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(call_count.get(), 5);
    assert_eq!(report.content, "done");
    assert_eq!(report.turn_reports.len(), 5);
    assert_eq!(report.tool_receipts.len(), 4);
}

#[test]
fn agent_turn_runtime_keeps_provider_tools_and_cache_key_stable_across_turns() {
    let input = ProviderStreamingTransportInput {
        client_channel: ClientChannel::Cli,
        turn_id: axon_domain::TurnId(1),
        provider_name: "test-provider".to_owned(),
        capabilities: axon_config::ProviderCapabilities::for_protocol(
            axon_config::PROVIDER_PROTOCOL_OPENAI,
        ),
        protocol: ProviderStreamProtocol::OpenAiResponses,
        base_url: "https://provider.example/v1".to_owned(),
        api_key_env: None,
        api_key: None,
        model_id: "test-model".to_owned(),
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
        structured_output: None,
        context_window: 32_000,
        supports_tools: true,
        supports_vision: false,
        messages: vec![AgentModelMessage::text("user", "ping")].into(),
        model_tools: vec![deferred_model_tool()],
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan: transport_prompt_cache_plan_for(
            ProviderStreamProtocol::OpenAiResponses,
            vec![deferred_model_tool()],
        ),
        anthropic_max_tokens: 4096,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
    };
    let call_count = std::cell::Cell::new(0_u8);
    let first_turn_body = std::cell::RefCell::new(serde_json::Value::Null);
    let later_turn_bodies = std::cell::RefCell::new(Vec::new());
    let _report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt == 0 {
                    first_turn_body.replace(
                        serde_json::from_str(&transport_request.request.body_text).unwrap(),
                    );
                    crate::test_stream_outcome("data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_search\",\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"db\\\"}\"}}\n\ndata: [DONE]\n")
                } else if attempt == 1 {
                    later_turn_bodies
                        .borrow_mut()
                        .push(transport_request.request.body_text.clone());
                    crate::test_stream_outcome("data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call_db\",\"name\":\"mcp__team__query_db\",\"arguments\":\"{\\\"query\\\":\\\"ping\\\"}\"}}\n\ndata: [DONE]\n")
                } else {
                    later_turn_bodies
                        .borrow_mut()
                        .push(transport_request.request.body_text.clone());
                    crate::test_stream_outcome("data: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\n\ndata: [DONE]\n")
                }
            },
            |_transport_report, _provider_history, _turn_id| {
                Ok(AgentTurnToolDecision::Continue {
                    receipts: vec!["search".to_owned()],
                    next_messages: vec![
                        AgentModelMessage::text("assistant", "").with_tool_calls(vec![
                            crate::AgentModelToolCallMetadata {
                                id: "call_search".to_owned(),
                                name: "builtin.search".to_owned(),
                                input: "{}".into(),
                            },
                        ]),
                        AgentModelMessage::tool_result(
                            "call_search",
                            "status=completed",
                            true,
                        ),
                    ],
                })
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(call_count.get(), 3);
    let later_turn_bodies = later_turn_bodies.borrow();
    assert_eq!(later_turn_bodies.len(), 2);
    let first_turn_body = first_turn_body.borrow();
    assert!(first_turn_body.to_string().contains("query_db"));
    assert!(
        later_turn_bodies
            .iter()
            .all(|body| body.contains("query_db"))
    );
    let second_body: serde_json::Value = serde_json::from_str(&later_turn_bodies[0]).unwrap();
    let third_body: serde_json::Value = serde_json::from_str(&later_turn_bodies[1]).unwrap();
    assert_eq!(first_turn_body["tools"], second_body["tools"]);
    assert_eq!(second_body["tools"], third_body["tools"]);
    assert_eq!(
        first_turn_body["prompt_cache_key"],
        second_body["prompt_cache_key"]
    );
    assert_eq!(
        second_body["prompt_cache_key"],
        third_body["prompt_cache_key"]
    );
}

#[test]
fn openai_responses_reuses_completed_server_state_with_full_http_history() {
    let mut input = test_transport_input();
    input.protocol = ProviderStreamProtocol::OpenAiResponses;
    input.capabilities =
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI);
    input.prompt_cache_plan =
        transport_prompt_cache_plan_for(ProviderStreamProtocol::OpenAiResponses, Vec::new());
    let call_count = std::cell::Cell::new(0_u8);
    let second_body = std::cell::RefCell::new(serde_json::Value::Null);

    let report = AgentTurnRuntime
        .run(
            &input,
            |request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt == 1 {
                    second_body.replace(
                        serde_json::from_str(&request.request.body_text).expect("请求体必须有效"),
                    );
                }
                if attempt == 0 {
                    crate::test_stream_outcome(
                        r#"data: {"type":"response.output_text.delta","delta":"继续"}
data: {"type":"response.completed","response":{"id":"resp-continue","end_turn":false}}
data: [DONE]
"#,
                    )
                } else {
                    crate::test_stream_outcome(
                        r#"data: {"type":"response.output_text.delta","delta":"完成"}
data: {"type":"response.completed","response":{"id":"resp-done","end_turn":true}}
data: [DONE]
"#,
                    )
                }
            },
            |_transport_report,
             _provider_history,
             _turn_id|
             -> AxonResult<AgentTurnToolDecision<String>> {
                unreachable!("无工具调用时不应执行工具")
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(report.content, "完成");
    assert_eq!(call_count.get(), 2);
    assert!(
        second_body.borrow()["input"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );
    assert!(second_body.borrow().get("previous_response_id").is_none());
}

fn transport_prompt_cache_plan_for(
    protocol: ProviderStreamProtocol,
    model_tools: Vec<ModelToolDefinition>,
) -> crate::ProviderPromptCachePlan {
    crate::ProviderPromptCachePlanRuntime
        .plan(crate::ProviderPromptCachePlanInput {
            provider_name: "test-provider".to_owned(),
            capabilities: axon_config::ProviderCapabilities::for_protocol(
                axon_config::PROVIDER_PROTOCOL_OPENAI,
            ),
            format: match protocol {
                ProviderStreamProtocol::OpenAiResponses => {
                    crate::PromptCacheProviderWireFormat::OpenAiResponses
                }
                ProviderStreamProtocol::OpenAiCompatible => {
                    crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat
                }
                ProviderStreamProtocol::AnthropicMessages => {
                    crate::PromptCacheProviderWireFormat::AnthropicMessages
                }
            },
            model_id: "test-model".to_owned(),
            context_window: 32_000,
            supports_tools: true,
            supports_vision: false,
            messages: vec![AgentModelMessage::text("user", "ping")].into(),
            model_tools,
            context_section_count: 1,
        })
        .unwrap()
}
