use super::*;
use crate::{
    AgentModelMessage, AgentRuntimeNoteKind, CanonicalCheckpoint, CompressionPolicy,
    ProviderStreamProtocol, ProviderStreamingTransportError, ProviderStreamingWireTransport,
    WindowRolloverPolicyRuntime, WindowRolloverRuntime,
};
use axon_client_identity::ClientChannel;

use super::test_support::*;

const STEER_TEST_CONTEXT_WINDOW: u64 = 4_096;
const STEER_OVERFLOW_CONTEXT_WINDOW: u64 = 256;

#[test]
fn agent_turn_runtime_records_turn_events_and_completes_without_tools() {
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
        structured_output: None,
    };
    let report = AgentTurnRuntime
        .run(
            &input,
            |_transport_request| crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n"),
            |_transport_report, _provider_history, _turn_id| Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                AgentTurnToolDecision::AwaitExternal { receipts: Vec::new() }
            ),
            None,
            None,
        )
        .unwrap();

    assert_eq!(report.content, "ok");
    assert_eq!(report.turn_reports.len(), 1);
    assert_eq!(report.turn_events.len(), 3);
    assert_eq!(report.turn_events[0].kind, AgentTurnEventKind::TurnStarted);
    assert_eq!(
        report.turn_events[0].part_id,
        "lifecycle:turn-started:turn-1"
    );
    assert_eq!(
        report.turn_events[1].kind,
        AgentTurnEventKind::ProviderCompleted
    );
    assert_eq!(
        report.turn_events[1].part_id,
        "lifecycle:provider-completed:turn-1"
    );
    assert_eq!(report.turn_events[2].kind, AgentTurnEventKind::Completed);
    assert_eq!(report.turn_events[2].part_id, "lifecycle:completed:turn-1");
}

#[test]
fn agent_turn_runtime_numbers_turns_from_the_input_turn_id() {
    let mut input = test_transport_input();
    input.turn_id = axon_domain::TurnId(7);

    let report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                assert_eq!(transport_request.turn_id, axon_domain::TurnId(7));
                crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
            },
            |_transport_report, _provider_history, _turn_id| Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                AgentTurnToolDecision::AwaitExternal { receipts: Vec::new() }
            ),
            None,
            None,
        )
        .unwrap();

    // 渠道 failover 会重跑整个 turn loop：起点由调用方给定，派生 part 身份才跨尝试唯一。
    assert_eq!(report.turn_reports[0].turn_id, axon_domain::TurnId(7));
    assert_eq!(
        report.turn_events[0].part_id,
        "lifecycle:turn-started:turn-7"
    );
    assert_eq!(report.turn_events[0].summary, "provider turn 7 started");
}

#[test]
fn unspecified_turn_id_keeps_one_based_turn_numbering() {
    let mut input = test_transport_input();
    input.turn_id = axon_domain::TurnId(0);

    let report = AgentTurnRuntime
        .run(
            &input,
            |_transport_request| crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n"),
            |_transport_report, _provider_history, _turn_id| Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                AgentTurnToolDecision::AwaitExternal { receipts: Vec::new() }
            ),
            None,
            None,
        )
        .unwrap();

    assert_eq!(report.turn_reports[0].turn_id, axon_domain::TurnId(1));
    assert_eq!(
        report.turn_events[0].part_id,
        "lifecycle:turn-started:turn-1"
    );
}

#[test]
fn agent_turn_runtime_continues_with_steer_after_provider_safe_boundary() {
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
        context_window: STEER_TEST_CONTEXT_WINDOW,
        supports_tools: true,
        supports_vision: false,
        messages: vec![AgentModelMessage::text("user", "第一条")].into(),
        model_tools: Vec::new(),
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan: transport_prompt_cache_plan(),
        anthropic_max_tokens: 64,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
        structured_output: None,
    };
    let source = TestSteerSource::from_input(AgentTurnSteerInput {
        message: AgentModelMessage::text("user", "补充要求"),
        quoted_message: axon_domain::QuotedMessageContext::new("om_parent", "父消息正文").ok(),
    });
    let request_bodies = std::cell::RefCell::new(Vec::new());
    let assistant_content = "a".repeat(2_048);
    let stream = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{assistant_content}\"}},\"finish_reason\":null}}]}}\n\ndata: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n"
    );

    let report = AgentTurnRuntime
        .run(
            &input,
            |transport_request| {
                request_bodies
                    .borrow_mut()
                    .push(transport_request.request.body_text.clone());
                crate::test_stream_outcome(&stream)
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
    assert_eq!(source.reopen_count.get(), 1);
    assert!(!request_bodies.borrow()[0].contains("父消息正文"));
    let second_body: serde_json::Value = serde_json::from_str(&request_bodies.borrow()[1]).unwrap();
    let messages = second_body["messages"].as_array().unwrap();
    assert!(messages.iter().any(|message| {
        message["role"] == "assistant" && message["content"] == assistant_content
    }));
    assert!(
        messages
            .iter()
            .any(|message| { message["role"] == "user" && message["content"] == "补充要求" })
    );
    assert!(messages.iter().any(|message| {
        message["role"] == "user"
            && message["content"].as_str().is_some_and(|content| {
                content.contains("om_parent") && content.contains("父消息正文")
            })
    }));
}

#[test]
fn oversized_steer_continuation_uses_the_unified_rollover_policy() {
    let mut input = test_transport_input();
    input.context_window = STEER_OVERFLOW_CONTEXT_WINDOW;
    let source = TestSteerSource::from_input(AgentTurnSteerInput {
        message: AgentModelMessage::text("user", "补充要求"),
        quoted_message: None,
    });
    let content = "a".repeat(2_048);
    let stream = format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n"
    );
    let mut requests = 0;
    let report = AgentTurnRuntime
        .run(
            &input,
            |_| {
                requests += 1;
                crate::test_stream_outcome(&stream)
            },
            |_, _, _| -> axon_domain::AxonResult<AgentTurnToolDecision<()>> {
                panic!("不应执行工具")
            },
            Some(&source),
            None,
        )
        .unwrap();
    assert_eq!(requests, 2);
    assert_eq!(report.turn_reports.len(), 2);
}

struct TestSteerSource {
    messages: std::cell::RefCell<Vec<AgentTurnSteerInput>>,
    reopen_count: std::cell::Cell<usize>,
}

impl TestSteerSource {
    fn from_input(input: AgentTurnSteerInput) -> Self {
        Self {
            messages: std::cell::RefCell::new(vec![input]),
            reopen_count: std::cell::Cell::new(0),
        }
    }

    fn from_message(message: AgentModelMessage) -> Self {
        Self::from_input(AgentTurnSteerInput {
            message,
            quoted_message: None,
        })
    }
}

impl AgentTurnSteerSource for TestSteerSource {
    fn drain(&self) -> Vec<AgentTurnSteerInput> {
        std::mem::take(&mut *self.messages.borrow_mut())
    }

    fn close_and_drain(&self) -> Vec<AgentTurnSteerInput> {
        self.drain()
    }

    fn reopen(&self) {
        self.reopen_count.set(self.reopen_count.get() + 1);
    }
}

#[test]
fn agent_turn_runtime_can_continue_into_second_turn() {
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
        structured_output: None,
    };
    let call_count = std::cell::Cell::new(0_u8);
    let report = AgentTurnRuntime
        .run(
            &input,
            |_transport_request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt == 0 {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_search\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"ping\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n")
                } else {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
                }
            },
            |transport_report, _provider_history, _turn_id| {
                if transport_report.runner_report.tool_calls.is_empty() {
                    Ok(AgentTurnToolDecision::AwaitExternal {
                        receipts: Vec::<String>::new(),
                    })
                } else {
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
                }
            },
            None,
            None,
        )
        .unwrap();

    assert_eq!(call_count.get(), 2);
    assert_eq!(report.content, "done");
    assert_eq!(report.tool_receipts, vec!["search".to_owned()]);
    assert_eq!(report.turn_reports.len(), 2);
}

#[test]
fn compaction_clears_server_state_for_openai_responses_over_http_and_websocket() {
    for wire_transport in [
        ProviderStreamingWireTransport::Http,
        ProviderStreamingWireTransport::WebSocket,
    ] {
        let mut input = test_transport_input();
        input.protocol = ProviderStreamProtocol::OpenAiResponses;
        input.wire_transport = wire_transport;
        input.capabilities =
            axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI);
        input.server_conversation_state_ref = Some("old-response".to_owned());
        input.messages = vec![
            AgentModelMessage::text("user", "继续当前任务"),
            AgentModelMessage::text("assistant", "x".repeat(8_000)),
            AgentModelMessage::text("assistant", "previous answer"),
        ]
        .into();
        input.prompt_cache_plan =
            transport_prompt_cache_plan_for(ProviderStreamProtocol::OpenAiResponses, Vec::new());
        let mut rollover = |messages: &[AgentModelMessage]| {
            CanonicalCheckpoint::parse(
                "P0:\nTASK:\n继续任务\nCONSTRAINTS:\n保持约束\nVERIFIED:\n已完成验证\nOPEN:\n继续下一步\nTOOLS:\n- none",
                messages,
            )
            .map_err(axon_domain::AxonError::runtime_failure)
        };
        let compression = WindowRolloverRuntime {
            runtime: WindowRolloverPolicyRuntime::new(CompressionPolicy {
                trigger_tokens: 2_000,
                context_window: 3_000,
            }),
            rollover: &mut rollover,
            last_projection_metrics: None,
        };
        let calls = std::cell::Cell::new(0_u8);
        let mut request_bodies = Vec::new();
        let report = AgentTurnRuntime
            .run(
                &input,
                |request| {
                    request_bodies.push(request.request.body_text.clone());
                    let call = calls.get();
                    calls.set(call + 1);
                    if call == 0 {
                        crate::test_stream_outcome(
                            "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"call-1\",\"name\":\"builtin_search\",\"arguments\":\"{}\"}}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"old-response\",\"end_turn\":false,\"usage\":{\"input_tokens\":3000,\"output_tokens\":1,\"total_tokens\":3001}}}\n",
                        )
                    } else {
                        crate::test_stream_outcome(
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"new-response\",\"end_turn\":true,\"usage\":{\"input_tokens\":20,\"output_tokens\":1,\"total_tokens\":21}}}\n",
                        )
                    }
                },
                |transport_report, _provider_history, _turn_id| {
                    if transport_report.runner_report.tool_calls.is_empty() {
                        Ok(AgentTurnToolDecision::AwaitExternal {
                            receipts: Vec::<String>::new(),
                        })
                    } else {
                        Ok(AgentTurnToolDecision::Continue {
                            receipts: Vec::new(),
                            next_messages: vec![AgentModelMessage::tool_result(
                                "call-1", "ok", true,
                            )],
                        })
                    }
                },
                None,
                Some(compression),
            )
            .expect("压缩后的 Responses turn 应完成");

        assert!(report.turn_events.iter().any(|event| {
            event.kind == AgentTurnEventKind::CompactionCompleted
                && event.status.as_deref() == Some("completed")
        }));
        let projection_event = report
            .turn_events
            .iter()
            .find(|event| {
                event.kind == AgentTurnEventKind::ProviderCompleted && event.turn_id.0 == 2
            })
            .expect("压缩后的 provider 事件应存在");
        assert!(
            projection_event
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("savedTokens"))
                .is_some_and(|value| value.is_number())
        );
        assert!(matches!(
            projection_event
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("breakReason"))
                .and_then(serde_json::Value::as_str),
            Some("conversation_rewritten" | "message_prefix_changed")
        ));
        assert!(request_bodies.len() >= 2);
        let second_body: serde_json::Value =
            serde_json::from_str(&request_bodies[1]).expect("请求体必须有效");
        assert!(second_body.get("previous_response_id").is_none());
    }
}

#[test]
fn agent_turn_runtime_sums_token_usage_across_tool_turns() {
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
        structured_output: None,
    };
    let call_count = std::cell::Cell::new(0_u8);
    let report = AgentTurnRuntime
        .run(
            &input,
            |_transport_request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt == 0 {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_search\",\"type\":\"function\",\"function\":{\"name\":\"builtin_search\",\"arguments\":\"{\\\"query\\\":\\\"ping\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":120,\"completion_tokens\":8,\"prompt_tokens_details\":{\"cached_tokens\":40,\"cache_write_tokens\":12}}}\n\ndata: [DONE]\n")
                } else {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":220,\"completion_tokens\":20,\"prompt_tokens_details\":{\"cached_tokens\":100}}}\n\ndata: [DONE]\n")
                }
            },
            |transport_report, _provider_history, _turn_id| {
                if transport_report.runner_report.tool_calls.is_empty() {
                    Ok(AgentTurnToolDecision::AwaitExternal {
                        receipts: Vec::<String>::new(),
                    })
                } else {
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
                }
            },
            None,
            None,
        )
        .unwrap();

    let usage = report.token_usage.expect("多轮 provider usage 应累计输出");

    assert_eq!(call_count.get(), 2);
    assert_eq!(usage.input_tokens, 188);
    assert_eq!(usage.cache_read_input_tokens, 140);
    assert_eq!(usage.cache_write_input_tokens, 12);
    assert_eq!(usage.complete_input_tokens(), 340);
    assert_eq!(usage.output_tokens, 28);
    assert_eq!(usage.total_tokens(), 368);
}

#[test]
fn agent_turn_runtime_records_provider_retry_event() {
    let input = ProviderStreamingTransportInput {
        client_channel: ClientChannel::Cli,
        turn_id: axon_domain::TurnId(1),
        provider_name: "test-provider".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        base_url: "https://provider.example/v1".to_owned(),
        api_key_env: None,
        api_key: None,
        model_id: "cpa/test-model".to_owned(),
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
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
        structured_output: None,
    };
    let call_count = std::cell::Cell::new(0_u8);

    let report = AgentTurnRuntime
        .run(
            &input,
            |_transport_request| {
                let attempt = call_count.get();
                call_count.set(attempt + 1);
                if attempt == 0 {
                    crate::ProviderStreamingTransportOutcome::Error(
                        ProviderStreamingTransportError::new("connection reset", None),
                    )
                } else {
                    crate::test_stream_outcome("data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n")
                }
            },
            |_transport_report, _provider_history, _turn_id| Ok::<AgentTurnToolDecision<String>, axon_domain::AxonError>(
                AgentTurnToolDecision::AwaitExternal { receipts: Vec::new() }
            ),
            None,
            None,
        )
        .unwrap();

    let retry = report
        .turn_events
        .iter()
        .find(|event| event.kind == AgentTurnEventKind::ProviderRetry)
        .expect("ProviderRetry event");

    assert_eq!(call_count.get(), 2);
    assert_eq!(retry.phase.as_deref(), Some("streaming"));
    assert_eq!(retry.part_id, "lifecycle:provider-retry:turn-1:attempt-2");
    assert_eq!(retry.status.as_deref(), Some("retrying"));
    assert_eq!(retry.tool_name.as_deref(), Some("cpa/test-model"));
    assert!(retry.summary.contains("瞬时传输错误"));
    assert_eq!(
        retry
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("attemptIndex"))
            .and_then(serde_json::Value::as_u64),
        Some(2),
    );
}

#[path = "agent_turn_runtime_continuation_tests.rs"]
mod agent_turn_runtime_continuation_tests;

#[path = "agent_turn_runtime_pre_final_guard_tests.rs"]
mod agent_turn_runtime_pre_final_guard_tests;
