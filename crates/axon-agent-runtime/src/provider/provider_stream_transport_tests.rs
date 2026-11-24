use super::*;
use crate::agent_model_tool_history::{
    IMAGE_CONTENT_OMITTED_PLACEHOLDER, PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED,
};
use crate::{
    AgentModelLocalImageRef, AgentModelMessage, PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED,
    ProviderStreamingTransportError, ProviderStructuredOutputContract,
    ProviderStructuredOutputMode,
};

#[test]
fn text_only_retry_rebuilds_image_free_provider_body() {
    let mut input = test_input();
    *input.messages = vec![
        AgentModelMessage::text("user", "分析历史图片").with_local_images(vec![
            AgentModelLocalImageRef::local("must-not-be-read.png", "image/png"),
        ]),
    ];
    input.prompt_cache_plan = provider_streaming_prompt_cache_plan(&input).unwrap();

    let mut calls = 0;
    let report = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            calls += 1;
            assert!(
                request
                    .request
                    .body_text
                    .contains(IMAGE_CONTENT_OMITTED_PLACEHOLDER)
            );
            assert!(!request.request.body_text.contains("must-not-be-read.png"));
            assert!(
                request
                    .request
                    .reasons
                    .iter()
                    .any(|reason| reason == PROVIDER_HISTORY_REASON_UNSUPPORTED_IMAGE_OMITTED)
            );
            if calls == 1 {
                ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                    "connection reset",
                    None,
                ))
            } else {
                crate::test_stream_outcome(openai_stream("pong"))
            }
        })
        .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(report.report.runner_report.content, "pong");
}

#[test]
fn retries_transient_error_with_new_attempt() {
    let input = test_input();
    let mut calls = 0;
    let report = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |_| {
            calls += 1;
            if calls == 1 {
                ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                    "connection reset",
                    None,
                ))
            } else {
                crate::test_stream_outcome(openai_stream("pong"))
            }
        })
        .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(report.report.attempt_count, 2);
    assert_eq!(report.report.runner_report.content, "pong");
    assert!(
        report
            .report
            .reasons
            .iter()
            .any(|reason| reason == PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED)
    );
}

#[test]
fn incomplete_stream_retries_with_distinct_attempt_indices() {
    let input = test_input();
    let mut calls = 0;
    let mut attempts = Vec::new();
    let report = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            calls += 1;
            attempts.push(request.attempt_index);
            if calls == 1 {
                ProviderStreamingTransportOutcome::Error(
                    ProviderStreamingTransportError::new(
                        "provider streaming 缺少完成标记",
                        Some(200),
                    )
                    .with_failure_kind(crate::ProviderStreamingTransportFailureKind::Incomplete),
                )
            } else {
                crate::test_stream_outcome(openai_stream("pong"))
            }
        })
        .unwrap();

    assert_eq!(attempts, vec![1, 2]);
    assert_eq!(report.report.runner_report.content, "pong");
}

#[test]
fn incomplete_stream_after_visible_output_fails_without_replaying_request() {
    let input = test_input();
    let mut calls = 0;

    let error = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |_| {
            calls += 1;
            ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new("provider streaming 缺少完成标记", Some(200))
                    .with_failure_kind(crate::ProviderStreamingTransportFailureKind::Incomplete),
            )
            .with_visible_output()
        })
        .expect_err("已投影的半截流不得重试");

    assert_eq!(calls, 1);
    assert!(format!("{error:?}").contains("缺少完成标记"));
}

#[test]
fn visible_transport_error_fails_without_replaying_request() {
    let input = test_input();
    let mut calls = 0;

    let error = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |_| {
            calls += 1;
            ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                "connection reset",
                None,
            ))
            .with_visible_output()
        })
        .expect_err("已投影后的 transport 错误不得重试");

    assert_eq!(calls, 1);
    assert!(format!("{error:?}").contains("connection reset"));
}

#[test]
fn incomplete_tool_call_retries_original_payload() {
    let input = test_input();
    let mut calls = 0;
    let mut first_body = None;
    let report = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            calls += 1;
            if calls == 1 {
                first_body = Some(request.request.body_text.clone());
                ProviderStreamingTransportOutcome::Error(
                    ProviderStreamingTransportError::new(
                        "provider tool-call 疑似上游截断",
                        Some(200),
                    )
                    .with_failure_kind(crate::ProviderStreamingTransportFailureKind::Incomplete),
                )
            } else {
                assert_eq!(request.request.body_text, first_body.clone().unwrap());
                crate::test_stream_outcome(openai_stream("pong"))
            }
        })
        .unwrap();

    assert_eq!(calls, 2);
    assert_eq!(report.report.runner_report.content, "pong");
}

#[test]
fn non_transient_error_fails_without_retry() {
    let input = test_input();
    let mut calls = 0;
    let error = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |_| {
            calls += 1;
            ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                "unauthorized",
                Some(401),
            ))
        })
        .unwrap_err();

    assert_eq!(calls, 1);
    assert!(format!("{error:?}").contains("401"));
}

#[test]
fn unsupported_reasoning_effort_downgrades_once_and_caches() {
    let mut input = test_input();
    input.provider_name = "effort-cache-provider".to_owned();
    input.model_id = "effort-cache-model".to_owned();
    input.reasoning_effort = Some("max".to_owned());
    let mut attempted = Vec::new();
    let report = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            let body: serde_json::Value = serde_json::from_str(&request.request.body_text).unwrap();
            attempted.push(body["reasoning_effort"].as_str().map(str::to_owned));
            if attempted.len() == 1 {
                ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                    r#"{\"error\":{\"message\":\"Unsupported value for this model\",\"param\":\"reasoning_effort\"}}"#,
                    Some(400),
                ))
            } else {
                crate::test_stream_outcome(openai_stream("pong"))
            }
        })
        .unwrap();

    assert_eq!(
        attempted,
        [Some("max".to_owned()), Some("xhigh".to_owned())]
    );
    assert_eq!(report.report.attempt_count, 2);
    assert!(report
        .report
        .reasons
        .iter()
        .any(|reason| reason == PROVIDER_STREAMING_TRANSPORT_REASON_REASONING_EFFORT_DOWNGRADED));
}

#[test]
fn server_state_reference_is_transport_gated() {
    let mut input = test_input();
    input.protocol = ProviderStreamProtocol::OpenAiResponses;
    input.capabilities =
        axon_config::ProviderCapabilities::for_protocol(axon_config::PROVIDER_PROTOCOL_OPENAI);
    input.prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: input.provider_name.clone(),
            capabilities: input.capabilities,
            format: PromptCacheProviderWireFormat::OpenAiResponses,
            model_id: input.model_id.clone(),
            context_window: input.context_window,
            supports_tools: input.supports_tools,
            supports_vision: input.supports_vision,
            messages: input.messages.clone(),
            model_tools: input.model_tools.clone(),
            context_section_count: input.context_section_count,
        })
        .unwrap();
    input.server_conversation_state_ref = Some("resp-continue".to_owned());

    let mut request_bodies = Vec::new();
    ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            request_bodies.push(
                serde_json::from_str::<serde_json::Value>(&request.request.body_text)
                    .expect("请求体必须有效"),
            );
            crate::test_stream_outcome(openai_stream("完成"))
        })
        .unwrap();
    assert!(request_bodies[0].get("previous_response_id").is_none());
    assert!(request_bodies[0]["input"].as_array().is_some());

    input.wire_transport = ProviderStreamingWireTransport::WebSocket;
    request_bodies.clear();
    ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            request_bodies.push(
                serde_json::from_str::<serde_json::Value>(&request.request.body_text)
                    .expect("请求体必须有效"),
            );
            crate::test_stream_outcome(openai_stream("完成"))
        })
        .unwrap();
    assert_eq!(request_bodies[0]["previous_response_id"], "resp-continue");
}

#[test]
fn all_protocols_drop_stale_server_state_after_stateless_rewrite() {
    for protocol in [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ] {
        for wire_transport in [
            ProviderStreamingWireTransport::Http,
            ProviderStreamingWireTransport::WebSocket,
        ] {
            let mut input = test_input();
            input.protocol = protocol;
            input.wire_transport = wire_transport;
            input.server_conversation_state_ref = match (protocol, wire_transport) {
                (
                    ProviderStreamProtocol::OpenAiResponses,
                    ProviderStreamingWireTransport::WebSocket,
                ) => None,
                _ => Some("stale-after-rewrite".to_owned()),
            };
            input.prompt_cache_plan = provider_streaming_prompt_cache_plan(&input).unwrap();
            let mut body = None;
            ProviderStreamingTransportRuntime
                .run_with_transport(&input, |request| {
                    body = Some(
                        serde_json::from_str::<serde_json::Value>(&request.request.body_text)
                            .expect("请求体必须有效"),
                    );
                    crate::test_stream_outcome(match protocol {
                        ProviderStreamProtocol::OpenAiResponses => {
                            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"done\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"end_turn\":true}}\n"
                        }
                        ProviderStreamProtocol::OpenAiCompatible => {
                            "data: {\"choices\":[{\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n"
                        }
                        ProviderStreamProtocol::AnthropicMessages => {
                            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"done\"}}\n\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\ndata: {\"type\":\"message_stop\"}\n"
                        }
                    })
                })
                .expect("各协议无状态请求应完成");
            assert!(
                body.as_ref()
                    .and_then(|value| value.get("previous_response_id"))
                    .is_none()
            );
        }
    }
}

#[test]
fn structured_output_downgrade_retries_with_prompt_only_request() {
    let mut input = test_input();
    input.provider_name = "structured-output-downgrade".to_owned();
    input.model_id = "deepseek-v4-flash".to_owned();
    input.structured_output = Some(ProviderStructuredOutputContract {
        schema: serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        }),
        mode: ProviderStructuredOutputMode::JsonSchema,
    });
    input.prompt_cache_plan = provider_streaming_prompt_cache_plan(&input).unwrap();

    let mut calls = 0;
    let mut bodies = Vec::new();
    let report = ProviderStreamingTransportRuntime
        .run_with_transport(&input, |request| {
            calls += 1;
            bodies.push(request.request.body_text.clone());
            if calls == 1 {
                ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                    r#"{"error":{"message":"Model DeepSeek-V4-Flash does not support JSON schema output mode","type":"invalid_request_error"}}"#,
                    Some(400),
                ))
            } else {
                crate::test_stream_outcome(openai_stream("{}"))
            }
        })
        .expect("json_schema 被拒绝后应降级重试并完成");

    assert_eq!(calls, 2);
    assert_eq!(report.report.attempt_count, 2);
    assert!(
        report.report.reasons.iter().any(
            |reason| reason == PROVIDER_STREAMING_TRANSPORT_REASON_STRUCTURED_OUTPUT_DOWNGRADED
        )
    );

    let first: serde_json::Value = serde_json::from_str(&bodies[0]).expect("首次请求体必须有效");
    assert_eq!(first["response_format"]["type"], "json_schema");
    let second: serde_json::Value = serde_json::from_str(&bodies[1]).expect("降级请求体必须有效");
    assert!(second.get("response_format").is_none());
    let system_text = second["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(system_text.contains("JSON Schema"));
    assert!(system_text.contains("answer"));
}

fn test_input() -> ProviderStreamingTransportInput {
    let messages = vec![AgentModelMessage::text("user", "ping")];
    let prompt_cache_plan = ProviderPromptCachePlanRuntime
        .plan(ProviderPromptCachePlanInput {
            provider_name: "local-test".to_owned(),
            capabilities: axon_config::ProviderCapabilities::stateless(),
            format: PromptCacheProviderWireFormat::OpenAiCompatibleChat,
            model_id: "test-model".to_owned(),
            context_window: 8_192,
            supports_tools: true,
            supports_vision: false,
            messages: messages.clone().into(),
            model_tools: Vec::new(),
            context_section_count: 1,
        })
        .unwrap();
    ProviderStreamingTransportInput {
        client_channel: axon_client_identity::ClientChannel::Cli,
        turn_id: TurnId(1),
        provider_name: "local-test".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        base_url: "http://127.0.0.1:11434/v1".to_owned(),
        api_key_env: None,
        api_key: None,
        model_id: "test-model".to_owned(),
        service_tier: None,
        reasoning_effort: None,
        reasoning_summary: Some("auto".to_owned()),
        context_window: 8_192,
        supports_tools: true,
        supports_vision: false,
        messages: messages.into(),
        model_tools: Vec::new(),
        context_composition: None,
        context_section_count: 1,
        prompt_cache_plan,
        anthropic_max_tokens: 8_192,
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        server_conversation_state_ref: None,
        structured_output: None,
    }
}

fn openai_stream(content: &str) -> String {
    format!(
        "data: {{\"choices\":[{{\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\ndata: [DONE]\n"
    )
}
