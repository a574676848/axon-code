use crate::{
    AgentModelMessage, PROVIDER_STREAMING_REASON_PROVIDER_ERROR,
    PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED, ProviderStreamProtocol,
    ProviderStreamingRunnerContext, ProviderStreamingRunnerDecision,
    ProviderStreamingRunnerRuntime, ProviderStreamingTransportError,
    ProviderStreamingTransportFailureKind, ProviderStreamingTransportOutcome,
    provider_streaming_retry_policy,
};

#[test]
fn provider_stream_protocol_parsing() {
    assert_eq!(
        ProviderStreamProtocol::from_code("openai").unwrap(),
        ProviderStreamProtocol::OpenAiResponses
    );
    assert_eq!(
        ProviderStreamProtocol::from_code("openai-chatcompletions").unwrap(),
        ProviderStreamProtocol::OpenAiCompatible
    );
    assert_eq!(
        ProviderStreamProtocol::from_code("anthropic").unwrap(),
        ProviderStreamProtocol::AnthropicMessages
    );
    assert!(ProviderStreamProtocol::from_code("dashscope").is_err());
    assert!(ProviderStreamProtocol::from_code("volcengine-ark-multimodal").is_err());
    assert!(ProviderStreamProtocol::from_code("unknown").is_err());
}

#[test]
fn structured_transport_error_separates_health_scopes() {
    assert_eq!(
        ProviderStreamingTransportError::new("upgrade required", Some(426)).failure_kind,
        ProviderStreamingTransportFailureKind::Capability
    );
    assert_eq!(
        ProviderStreamingTransportError::new("too many requests", Some(429)).failure_kind,
        ProviderStreamingTransportFailureKind::RateLimited
    );
    assert_eq!(
        ProviderStreamingTransportError::new("service unavailable", Some(503)).failure_kind,
        ProviderStreamingTransportFailureKind::Upstream
    );
    assert_eq!(
        ProviderStreamingTransportError::new("connection reset", None).failure_kind,
        ProviderStreamingTransportFailureKind::Connection
    );
    assert_eq!(
        ProviderStreamingTransportError::new("provider streaming 已取消", None).failure_kind,
        ProviderStreamingTransportFailureKind::Cancelled
    );
    assert_eq!(
        ProviderStreamingTransportError::new(
            "The input token count exceeds the maximum number of tokens allowed (1048576).",
            Some(400),
        )
        .failure_kind,
        ProviderStreamingTransportFailureKind::ContextOverflow
    );
}

#[test]
fn anthropic_stream_errors_retry_with_original_messages() {
    for err_msg in [
        "provider tool-call 疑似上游截断",
        "Anthropic Messages stream JSON 无效",
    ] {
        let mut context = test_context();
        context.protocol = ProviderStreamProtocol::AnthropicMessages;
        context.supports_tools = true;
        let runner = ProviderStreamingRunnerRuntime;
        let plan = runner.plan_initial(&context).unwrap();
        let decision = runner
            .complete_attempt(&context, plan, protocol_stream_error(err_msg))
            .unwrap();

        match decision {
            ProviderStreamingRunnerDecision::Retry(plan) => {
                assert_eq!(
                    plan.transient_retry_budget_remaining,
                    provider_streaming_retry_policy().stream_max_retries - 1
                );
                assert!(has_reason(
                    &plan.reasons,
                    PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED
                ));
            }
            _ => panic!("Anthropic 流异常应复用原始输入重试: {err_msg}"),
        }
    }
}

#[test]
fn empty_openai_tool_call_arguments_are_retryable() {
    let mut context = test_context();
    context.supports_tools = true;
    let runner = ProviderStreamingRunnerRuntime;
    let plan = runner.plan_initial(&context).unwrap();
    let decision = runner
        .complete_attempt(
            &context,
            plan,
            protocol_stream_error("provider tool-call 疑似上游截断"),
        )
        .unwrap();

    match decision {
        ProviderStreamingRunnerDecision::Retry(plan) => {
            assert_eq!(
                plan.transient_retry_budget_remaining,
                provider_streaming_retry_policy().stream_max_retries - 1
            );
            assert!(has_reason(
                &plan.reasons,
                PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED
            ));
        }
        ProviderStreamingRunnerDecision::Completed(_)
        | ProviderStreamingRunnerDecision::Failed(_) => {
            panic!("空对象 tool-call arguments 应触发重试")
        }
    }
}

#[test]
fn transient_retry_budget_exhaustion_fails_closed() {
    let context = test_context();
    let runner = ProviderStreamingRunnerRuntime;
    let mut plan = runner.plan_initial(&context).unwrap();
    plan.transient_retry_budget_remaining = 0;

    let decision = runner
        .complete_attempt(
            &context,
            plan,
            ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
                "connection reset",
                None,
            )),
        )
        .unwrap();

    match decision {
        ProviderStreamingRunnerDecision::Failed(error) => {
            assert!(error.message.contains("transient retry budget exhausted"));
            assert!(
                error
                    .message
                    .contains(PROVIDER_STREAMING_REASON_PROVIDER_ERROR)
            );
        }
        ProviderStreamingRunnerDecision::Completed(_)
        | ProviderStreamingRunnerDecision::Retry(_) => {
            panic!("瞬时错误预算耗尽后必须 fail closed")
        }
    }
}

#[test]
fn tool_calls_without_protocol_termination_retry_for_all_protocols() {
    let protocols = [
        ProviderStreamProtocol::OpenAiResponses,
        ProviderStreamProtocol::OpenAiCompatible,
        ProviderStreamProtocol::AnthropicMessages,
    ];

    for protocol in protocols {
        let mut context = test_context();
        context.protocol = protocol;
        context.supports_tools = true;
        let runner = ProviderStreamingRunnerRuntime;
        let plan = runner.plan_initial(&context).unwrap();

        match runner
            .complete_attempt(
                &context,
                plan,
                incomplete_stream_error("provider streaming 缺少完成标记"),
            )
            .unwrap()
        {
            ProviderStreamingRunnerDecision::Retry(plan) => {
                assert_eq!(
                    plan.transient_retry_budget_remaining,
                    provider_streaming_retry_policy().stream_max_retries - 1
                );
                assert!(has_reason(
                    &plan.reasons,
                    PROVIDER_STREAMING_REASON_TRANSIENT_RETRY_PLANNED
                ));
            }
            ProviderStreamingRunnerDecision::Completed(_)
            | ProviderStreamingRunnerDecision::Failed(_) => {
                panic!("缺少协议终止事件的工具调用必须按不完整流重试")
            }
        }
    }
}

fn incomplete_stream_error(message: &str) -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(
        ProviderStreamingTransportError::new(message, Some(200))
            .with_failure_kind(ProviderStreamingTransportFailureKind::Incomplete),
    )
}

fn protocol_stream_error(message: &str) -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(
        ProviderStreamingTransportError::new(message, Some(200))
            .with_failure_kind(ProviderStreamingTransportFailureKind::Incomplete),
    )
}

fn has_reason(reasons: &[String], expected: &str) -> bool {
    reasons.iter().any(|reason| reason == expected)
}

fn test_context() -> ProviderStreamingRunnerContext {
    let messages = vec![AgentModelMessage {
        role: "user".to_owned(),
        content: "ping".to_owned(),
        provider_metadata: None,
        local_images: Vec::new(),
        local_files: Vec::new(),
        cache_excluded: false,
        source: crate::AgentModelMessageSource::Conversation,
        kind: crate::AgentModelMessageKind::Regular,
        retention_class: crate::RetentionClass::P0Contract,
        source_ref: None,
    }];
    ProviderStreamingRunnerContext {
        provider_name: "test-provider".to_owned(),
        capabilities: axon_config::ProviderCapabilities::stateless(),
        protocol: ProviderStreamProtocol::OpenAiCompatible,
        model_id: "test-model".to_owned(),
        context_window: 8_192,
        supports_tools: false,
        supports_vision: false,
        messages: messages.clone().into(),
        model_tools: Vec::new(),
        context_section_count: 1,
        prompt_cache_plan: crate::ProviderPromptCachePlanRuntime
            .plan(crate::ProviderPromptCachePlanInput {
                provider_name: "test-provider".to_owned(),
                capabilities: axon_config::ProviderCapabilities::stateless(),
                format: crate::PromptCacheProviderWireFormat::OpenAiCompatibleChat,
                model_id: "test-model".to_owned(),
                context_window: 8_192,
                supports_tools: false,
                supports_vision: false,
                messages: messages.into(),
                model_tools: Vec::new(),
                context_section_count: 1,
            })
            .unwrap(),
        server_conversation_state_ref: None,
    }
}
