//! Provider 流式收尾投影的用量与生命周期回归。

use super::*;
use axon_protocol::wire::WireEventKind;

#[test]
fn streaming_lifecycle_pairs_successful_attempts_and_marks_retry_model() {
    let plan = ProviderTransportPlan {
        provider_name: "cpa".to_owned(),
        model_id: "cpa/gpt-5.4".to_owned(),
        logical_session: "test-session".to_owned(),
        endpoint: "https://example.com/v1".to_owned(),
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        stream_protocol: ProviderStreamProtocol::OpenAiCompatible,
        timeout: Duration::from_secs(1),
    };
    let mut events = Vec::new();
    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "test-session".to_owned());

        emit_streaming_started(&plan, ProviderStreamingWireTransport::Http, Some(&sink), 1);
        emit_streaming_completed(
            &plan,
            Some(&sink),
            1,
            &ProviderStreamingTransportOutcome::StreamCompleted { http_status: 200 },
            None,
        );
        emit_streaming_started(&plan, ProviderStreamingWireTransport::Http, Some(&sink), 2);
        emit_streaming_completed(
            &plan,
            Some(&sink),
            2,
            &ProviderStreamingTransportOutcome::VisibleOutput(Box::new(
                ProviderStreamingTransportOutcome::StreamCompleted { http_status: 200 },
            )),
            None,
        );
        emit_streaming_started(&plan, ProviderStreamingWireTransport::Http, Some(&sink), 3);
        emit_streaming_completed(
            &plan,
            Some(&sink),
            3,
            &interrupted_transport_outcome(),
            None,
        );
    }

    let lifecycle_attempts = |kind: &str| {
        events
            .iter()
            .filter_map(|event| match &event.event.kind {
                WireEventKind::TurnLifecycle(lifecycle)
                    if lifecycle.kind.as_deref() == Some(kind) =>
                {
                    lifecycle.attempt_index
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let started_attempts = lifecycle_attempts("StreamingStarted");
    let completed_attempts = lifecycle_attempts("StreamingCompleted");
    let lifecycle = events
        .iter()
        .filter_map(|event| match &event.event.kind {
            WireEventKind::TurnLifecycle(view) if view.kind.as_deref() == Some("ProviderRetry") => {
                Some(view)
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(started_attempts, completed_attempts);
    assert_eq!(completed_attempts, vec![1, 2, 3]);
    assert_eq!(lifecycle.len(), 2);
    assert_eq!(lifecycle[0].kind.as_deref(), Some("ProviderRetry"));
    assert_eq!(lifecycle[0].tool_name.as_deref(), Some("cpa/gpt-5.4"));
    assert_eq!(lifecycle[0].attempt_index, Some(2));
    assert_eq!(lifecycle[1].attempt_index, Some(3));
}

#[test]
fn streaming_completed_follows_final_usage_snapshot() {
    let plan = ProviderTransportPlan {
        provider_name: "cpa".to_owned(),
        model_id: "cpa/gpt-5.4".to_owned(),
        logical_session: "usage-order-session".to_owned(),
        endpoint: "https://example.com/v1".to_owned(),
        wire_transport: ProviderStreamingWireTransport::Http,
        websocket_url: None,
        enable_request_compression: false,
        stream_protocol: ProviderStreamProtocol::OpenAiCompatible,
        timeout: Duration::from_secs(1),
    };
    let mut events = Vec::new();
    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "usage-order-session".to_owned());
        let outcome = ProviderStreamingTransportOutcome::AggregatedStream {
            http_status: 200,
            stream: Box::new(axon_agent_runtime::ProviderStreamAggregate {
                deltas: Vec::new(),
                end_turn: Some(true),
                provider_event_count: 1,
                token_usage: Some(axon_agent_runtime::TokenUsageTelemetry {
                    input_tokens: 10,
                    output_tokens: 4,
                    total_input_tokens: 10,
                    ..Default::default()
                }),
                reasoning_content: None,
                tool_calls: Vec::new(),
                server_conversation_state_ref: None,
                max_tokens_truncated: false,
            }),
        };
        emit_streaming_started(&plan, ProviderStreamingWireTransport::Http, Some(&sink), 1);
        emit_streaming_completed(
            &plan,
            Some(&sink),
            1,
            &outcome,
            Some(axon_agent_runtime::ContextTokenComposition {
                system_prompt_tokens: 1_800,
                tool_definition_tokens: 19_100,
                conversation_tokens: 6_200,
            }),
        );
    }
    let usage_index = events
        .iter()
        .position(|event| matches!(event.event.kind, WireEventKind::UsageUpdate(_)))
        .expect("最终 usage 必须投影");
    // 构成必须搭同一帧用量投出去：单独算出来却没人发，界面就永远是空的。
    let WireEventKind::UsageUpdate(usage) = &events[usage_index].event.kind else {
        panic!("usage 事件类型应保持一致");
    };
    assert_eq!(usage.system_prompt_tokens, 1_800);
    assert_eq!(usage.tool_definition_tokens, 19_100);
    assert_eq!(usage.conversation_tokens, 6_200);
    let completed_index = events
        .iter()
        .position(|event| {
            matches!(
                &event.event.kind,
                WireEventKind::TurnLifecycle(lifecycle)
                    if lifecycle.kind.as_deref() == Some("StreamingCompleted")
            )
        })
        .expect("StreamingCompleted 必须投影");
    assert!(usage_index < completed_index);
}
