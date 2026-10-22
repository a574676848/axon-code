use std::cell::{Cell, RefCell};

use axon_client_identity::ClientChannel;
use axon_config::{ChannelBalancerConfig, ModelConfig, ProviderConfig};

use super::*;

const ROUTING_KEY: &str = "gpt-4o";
const SESSION_ID: &str = "session-failover";

fn provider(name: &str) -> ProviderConfig {
    ProviderConfig::new(name, axon_config::PROVIDER_PROTOCOL_OPENAI, "https://x/v1")
}

fn model(id: &str, provider_name: &str) -> ModelConfig {
    ModelConfig {
        name: ROUTING_KEY.to_owned(),
        id: id.to_owned(),
        provider: provider_name.to_owned(),
        context_window: 128_000,
        ..ModelConfig::default()
    }
}

fn chain(specs: &[(&str, &str)]) -> Vec<(ModelConfig, ProviderConfig)> {
    specs
        .iter()
        .map(|(model_id, provider_name)| (model(model_id, provider_name), provider(provider_name)))
        .collect()
}

fn ok_response(provider_name: &str, model_id: &str) -> AgentRunResponse<String> {
    AgentRunResponse {
        content: "ok".to_owned(),
        assistant_part_id: "assistant-part:1".to_owned(),
        http_status: 200,
        token_usage: None,
        tool_receipts: Vec::new(),
        tool_calls: Vec::new(),
        provider: provider_name.to_owned(),
        model: model_id.to_owned(),
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        turn_events: Vec::new(),
        max_tokens_truncated: false,
    }
}

fn transport_error(http_status: Option<u16>, message: &str) -> Box<AgentRunExecutionError<String>> {
    Box::new(AgentRunExecutionError {
        message: message.to_owned(),
        http_status,
        error_type: AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT,
        error_code: None,
        provider_protocol: None,
        error_param: None,
        failure_kind: None,
        call_index: None,
        provider_error: None,
        token_usage: None,
        tool_receipts: Box::new([]),
        visible_output_emitted: false,
        channel_attempts: Vec::new(),
        provider_event_type: None,
        upstream_request_id: None,
        committed_side_effects: false,
    })
}

fn visible_transport_error(
    http_status: Option<u16>,
    message: &str,
) -> Box<AgentRunExecutionError<String>> {
    let mut error = *transport_error(http_status, message);
    error.visible_output_emitted = true;
    Box::new(error)
}

/// 已投影可见输出、且已经写过文件的传输失败：副作用不可重放。
fn visible_transport_error_with_writes(
    http_status: Option<u16>,
    message: &str,
) -> Box<AgentRunExecutionError<String>> {
    let mut error = *visible_transport_error(http_status, message);
    error.committed_side_effects = true;
    Box::new(error)
}

struct RunTrace {
    result: Result<AgentRunResponse<String>, Box<AgentRunExecutionError<String>>>,
    successes: Vec<String>,
    failures: Vec<String>,
    affinities: Vec<String>,
    channel_events: Vec<AgentRunChannelEvent>,
}

fn run(
    chain: &[(ModelConfig, ProviderConfig)],
    execute: impl FnMut(
        &ModelConfig,
        &ProviderConfig,
    ) -> Result<AgentRunResponse<String>, Box<AgentRunExecutionError<String>>>,
) -> RunTrace {
    let successes = RefCell::new(Vec::new());
    let failures = RefCell::new(Vec::new());
    let affinities = RefCell::new(Vec::new());
    let channel_events = RefCell::new(Vec::new());
    let result = run_agent_channel_failover(
        AgentRunFailoverPlan {
            chain,
            routing_key: ROUTING_KEY,
            session_id: SESSION_ID,
            balancer: ChannelBalancerConfig::default(),
            now_ms: 1_000,
        },
        |key| successes.borrow_mut().push(key.provider.clone()),
        |key, _now| failures.borrow_mut().push(key.provider.clone()),
        |_routing_key, _session_id, provider, _now, _ttl_ms| {
            affinities.borrow_mut().push(provider.to_owned());
        },
        |event| channel_events.borrow_mut().push(event),
        execute,
    );
    RunTrace {
        result,
        successes: successes.into_inner(),
        failures: failures.into_inner(),
        affinities: affinities.into_inner(),
        channel_events: channel_events.into_inner(),
    }
}

#[test]
fn transient_failure_switches_to_next_channel() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let calls = Cell::new(0u32);
    let trace = run(&chain, |_model, provider| {
        calls.set(calls.get() + 1);
        if provider.name == "p-a" {
            Err(transport_error(Some(503), "service unavailable"))
        } else {
            Ok(ok_response(&provider.name, "m-b"))
        }
    });
    let response = trace.result.expect("第二渠道应成功");
    assert_eq!(calls.get(), 2);
    assert_eq!(response.provider, "p-b");
    assert_eq!(response.channel_attempts.len(), 2);
    assert_eq!(response.channel_attempts[0].outcome, "transient_failure");
    assert_eq!(response.channel_attempts[1].outcome, "success");
    assert_eq!(trace.failures, vec!["p-a"]);
    assert_eq!(trace.successes, vec!["p-b"]);
    assert_eq!(trace.affinities, vec!["p-b"]);
    assert_eq!(
        trace
            .channel_events
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            AgentRunChannelEventKind::AttemptStarted,
            AgentRunChannelEventKind::AttemptFailed,
            AgentRunChannelEventKind::Failover,
            AgentRunChannelEventKind::AttemptStarted,
        ]
    );
    assert_eq!(
        trace.channel_events[2].next_provider.as_deref(),
        Some("p-b")
    );
}

#[test]
fn permanent_failure_does_not_switch() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let calls = Cell::new(0u32);
    let trace = run(&chain, |_model, _provider| {
        calls.set(calls.get() + 1);
        Err(transport_error(Some(401), "unauthorized"))
    });
    let error = trace.result.expect_err("永久错误应快速失败");
    assert_eq!(calls.get(), 1);
    assert_eq!(error.channel_attempts.len(), 1);
    assert_eq!(error.channel_attempts[0].outcome, "permanent_failure");
    assert!(trace.successes.is_empty());
    // 传输类永久失败（401）必须供数熔断器：不供数则坏渠道永远保持“健康”。
    assert_eq!(trace.failures, vec!["p-a"]);
    assert!(trace.affinities.is_empty());
}

#[test]
fn all_channels_exhausted_returns_aggregated_error() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let trace = run(&chain, |_model, _provider| {
        Err(transport_error(Some(503), "down"))
    });
    let error = trace.result.expect_err("全链耗尽应返回错误");
    assert_eq!(error.channel_attempts.len(), 2);
    assert!(error.message.contains("全部 2 个渠道均失败"));
    assert_eq!(trace.failures, vec!["p-a", "p-b"]);
    assert!(trace.affinities.is_empty());
}

#[test]
fn non_transport_error_is_permanent() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let calls = Cell::new(0u32);
    let trace = run(&chain, |_model, _provider| {
        calls.set(calls.get() + 1);
        Err(Box::new(AgentRunExecutionError {
            message: "tool loop exhausted".to_owned(),
            http_status: Some(200),
            error_type: AGENT_RUN_ERROR_TYPE_RUNTIME,
            error_code: None,
            provider_protocol: None,
            error_param: None,
            failure_kind: None,
            call_index: None,
            provider_error: None,
            token_usage: None,
            tool_receipts: Box::new([]),
            visible_output_emitted: false,
            channel_attempts: Vec::new(),
            provider_event_type: None,
            upstream_request_id: None,
            committed_side_effects: false,
        }))
    });
    trace.result.expect_err("非传输错误不应换渠道");
    assert_eq!(calls.get(), 1);
    assert!(trace.failures.is_empty());
}

#[test]
fn provider_error_projection_parses_stream_error_payload() {
    let message = format!(
        "{PROVIDER_STREAM_ERROR_EVENT_PREFIX}{{\"error\":{{\"type\":\"invalid_request_error\",\"code\":\"context_length_exceeded\",\"param\":\"messages\",\"message\":\"too long\"}}}}；reason=overflow"
    );
    let projection = agent_run_provider_error_projection(&message).expect("provider error");
    assert_eq!(
        projection.error_type.as_deref(),
        Some("invalid_request_error")
    );
    assert_eq!(projection.code.as_deref(), Some("context_length_exceeded"));
    assert_eq!(projection.param.as_deref(), Some("messages"));
    assert_eq!(projection.message, "too long");
}

#[test]
fn axon_provider_transport_error_maps_to_run_execution_error() {
    let error = axon_domain::AxonError::provider_transport(
        "{\"error\":{\"code\":\"bad_request\",\"message\":\"bad input\"}}",
        Some(400),
    );
    let execution_error = agent_run_execution_error_from_axon_error(&error, &["receipt"]);
    assert_eq!(
        execution_error.error_type,
        AGENT_RUN_ERROR_TYPE_PROVIDER_TRANSPORT
    );
    assert_eq!(execution_error.http_status, Some(400));
    assert_eq!(execution_error.error_code.as_deref(), Some("bad_request"));
    assert_eq!(execution_error.message, "bad input");
    assert_eq!(execution_error.tool_receipts.as_ref(), ["receipt"]);

    let config_error = axon_domain::AxonError::Config(
        axon_domain::ConfigError::InvalidConfiguration("provider protocol 无效".to_owned()),
    );
    let config_exec_error = agent_run_execution_error_from_axon_error(&config_error, &["receipt"]);
    assert_eq!(
        config_exec_error.error_type,
        AGENT_RUN_ERROR_TYPE_CONFIGURATION
    );
    assert!(config_exec_error.message.contains("provider protocol 无效"));
}

#[test]
fn run_response_from_turn_report_preserves_runtime_fields() {
    let response = agent_run_response_from_turn_report(AgentRunCompletedInput {
        report: AgentTurnRuntimeReport {
            content: "done".to_owned(),
            http_status: 200,
            token_usage: None,
            tool_receipts: vec!["receipt".to_owned()],
            turn_reports: vec![AgentTurnIterationReport {
                turn_id: axon_domain::TurnId(7),
                transport_report: crate::ProviderStreamingTransportReport {
                    runner_report: crate::ProviderStreamingRunnerReport {
                        deltas: Vec::new(),
                        content: "done".to_owned(),
                        end_turn: Some(true),
                        http_status: 200,
                        provider_event_count: 1,
                        token_usage: None,
                        reasoning_content: Some("分析路径".to_owned()),
                        tool_calls: Vec::new(),
                        server_conversation_state_ref: None,
                        max_tokens_truncated: false,
                        remote_provider_payload_allowed: false,
                        reasons: Vec::new(),
                    },
                    cache_shape: crate::ProviderVisibleCacheShape::from_serialized_request(
                        crate::ProviderStreamProtocol::OpenAiCompatible,
                        r#"{"model":"model-a","tools":[],"messages":[]}"#,
                    )
                    .unwrap(),
                    attempt_count: 1,
                    remote_request_body_allowed: false,
                    remote_provider_payload_allowed: false,
                    reasons: Vec::new(),
                },
            }],
            turn_events: Vec::new(),
            max_tokens_truncated: false,
        },
        provider: "provider-a".to_owned(),
        model: "model-a".to_owned(),
        turn_events: Vec::new(),
    });

    assert_eq!(response.content, "done");
    assert_eq!(response.assistant_part_id, "assistant-part:7");
    assert_eq!(response.http_status, 200);
    assert_eq!(response.provider, "provider-a");
    assert_eq!(response.model, "model-a");
    assert_eq!(response.tool_receipts, vec!["receipt"]);
    assert_eq!(response.reasoning_messages.len(), 1);
    assert_eq!(response.reasoning_messages[0].part_id, "reasoning-part:7");
    assert_eq!(
        response.reasoning_messages[0].reasoning_id,
        agent_run_provider_reasoning_id(7)
    );
    assert_ne!(
        response.reasoning_messages[0].part_id,
        response.reasoning_messages[0].reasoning_id
    );
    assert_eq!(response.reasoning_messages[0].content, "分析路径");
    assert!(response.channel_attempts.is_empty());
}

#[test]
fn wire_transport_config_maps_to_runtime_transport() {
    assert_eq!(
        agent_run_wire_transport_from_config::<String>(axon_config::PROVIDER_WIRE_TRANSPORT_HTTP)
            .expect("http transport"),
        ProviderStreamingWireTransport::Http
    );
    assert_eq!(
        agent_run_wire_transport_from_config::<String>(
            axon_config::PROVIDER_WIRE_TRANSPORT_WEBSOCKET
        )
        .expect("websocket transport"),
        ProviderStreamingWireTransport::WebSocket
    );
    let error = agent_run_wire_transport_from_config::<String>("stdio")
        .expect_err("invalid transport should fail");
    assert_eq!(error.error_type, AGENT_RUN_ERROR_TYPE_CONFIGURATION);
}

#[test]
fn provider_streaming_transport_input_is_built_from_channel_config() {
    let model = model("model-a", "provider-a");
    let mut provider = provider("provider-a");
    provider.enable_request_compression = true;
    let input =
        agent_run_provider_streaming_transport_input::<String>(AgentRunProviderTransportInput {
            client_channel: ClientChannel::Cli,
            model: &model,
            provider: &provider,
            reasoning_effort: None,
            messages: vec![AgentModelMessage::text("user", "hello")],
            model_tools: Vec::new(),
            context_section_count: 2,
            structured_output: None,
        })
        .expect("transport input");

    assert_eq!(input.provider_name, "provider-a");
    assert_eq!(input.model_id, "model-a");
    assert_eq!(input.context_section_count, 2);
    assert_eq!(input.anthropic_max_tokens, 0);
    assert_eq!(input.wire_transport, ProviderStreamingWireTransport::Http);
    assert!(input.enable_request_compression);
    assert_eq!(input.messages.len(), 1);
    assert_eq!(input.messages[0].content, "hello");
}

#[test]
fn anthropic_transport_uses_model_output_capability() {
    let mut model = model("claude", "anthropic");
    model.max_output_tokens = Some(16_384);
    let mut provider = provider("anthropic");
    provider.protocol = axon_config::PROVIDER_PROTOCOL_ANTHROPIC.to_owned();

    let input =
        agent_run_provider_streaming_transport_input::<String>(AgentRunProviderTransportInput {
            client_channel: ClientChannel::Cli,
            model: &model,
            provider: &provider,
            reasoning_effort: None,
            messages: vec![AgentModelMessage::text("user", "hello")],
            model_tools: Vec::new(),
            context_section_count: 0,
            structured_output: None,
        })
        .expect("Anthropic transport 应读取模型输出能力");

    assert_eq!(input.anthropic_max_tokens, 16_384);

    model.max_output_tokens = None;
    let input =
        agent_run_provider_streaming_transport_input::<String>(AgentRunProviderTransportInput {
            client_channel: ClientChannel::Cli,
            model: &model,
            provider: &provider,
            reasoning_effort: None,
            messages: vec![AgentModelMessage::text("user", "hello")],
            model_tools: Vec::new(),
            context_section_count: 0,
            structured_output: None,
        })
        .expect("缺少 Anthropic 输出能力时应使用统一默认值");
    assert_eq!(
        input.anthropic_max_tokens,
        axon_config::DEFAULT_MODEL_MAX_OUTPUT_TOKENS
    );
}

#[test]
fn provider_protocol_validation_accepts_chat_capable_protocols() {
    let mut provider = provider("provider-a");
    assert!(validate_agent_run_provider_protocol(&provider).is_ok());

    provider.protocol = "dashscope".to_owned();
    assert!(validate_agent_run_provider_protocol(&provider).is_err());
}

#[test]
fn visible_output_transient_failure_switches_channel_without_side_effects() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let calls = Cell::new(0u32);
    let trace = run(&chain, |_model, provider| {
        calls.set(calls.get() + 1);
        if provider.name == "p-a" {
            Err(visible_transport_error(Some(503), "connection reset"))
        } else {
            Ok(ok_response(&provider.name, "m-b"))
        }
    });

    let response = trace
        .result
        .expect("瞬时传输失败且无不可重放副作用时应续跑到下一条渠道");
    assert_eq!(calls.get(), 2);
    assert_eq!(response.provider, "p-b");
    assert_eq!(response.channel_attempts.len(), 2);
    assert_eq!(response.channel_attempts[0].outcome, OUTCOME_VISIBLE_OUTPUT);
    assert_eq!(response.channel_attempts[1].outcome, "success");
    assert_eq!(trace.failures, vec!["p-a"]);
    assert_eq!(trace.successes, vec!["p-b"]);
}

#[test]
fn visible_output_failure_stops_after_committed_side_effects() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let calls = Cell::new(0u32);
    let trace = run(&chain, |_model, _provider| {
        calls.set(calls.get() + 1);
        Err(visible_transport_error_with_writes(
            Some(503),
            "connection reset",
        ))
    });

    let error = trace
        .result
        .expect_err("已提交副作用的失败必须停下等人工裁决");
    assert_eq!(calls.get(), 1);
    assert!(error.visible_output_emitted);
    assert!(error.committed_side_effects);
    assert_eq!(error.channel_attempts.len(), 1);
    assert_eq!(error.channel_attempts[0].outcome, OUTCOME_VISIBLE_OUTPUT);
    assert_eq!(trace.failures, vec!["p-a"]);
}

#[test]
fn visible_request_body_invalid_switches_to_next_channel() {
    let chain = chain(&[("m-a", "p-a"), ("m-b", "p-b")]);
    let calls = Cell::new(0u32);
    let trace = run(&chain, |_model, provider| {
        calls.set(calls.get() + 1);
        if provider.name == "p-a" {
            Err(visible_transport_error(
                Some(400),
                "Invalid request (REQUEST_BODY_INVALID): Invalid tool use format.",
            ))
        } else {
            Ok(ok_response(&provider.name, "m-b"))
        }
    });

    let response = trace.result.expect("请求体格式错误应切换渠道");
    assert_eq!(calls.get(), 2);
    assert_eq!(response.provider, "p-b");
    assert_eq!(response.channel_attempts.len(), 2);
    assert_eq!(response.channel_attempts[0].outcome, OUTCOME_VISIBLE_OUTPUT);
    assert_eq!(response.channel_attempts[1].outcome, "success");
    assert_eq!(trace.failures, vec!["p-a"]);
    assert_eq!(trace.successes, vec!["p-b"]);
    assert_eq!(
        trace
            .channel_events
            .iter()
            .map(|event| event.kind)
            .collect::<Vec<_>>(),
        vec![
            AgentRunChannelEventKind::AttemptStarted,
            AgentRunChannelEventKind::AttemptFailed,
            AgentRunChannelEventKind::Failover,
            AgentRunChannelEventKind::AttemptStarted,
        ]
    );
}
