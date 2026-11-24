//! 一次渠道传输尝试的执行与状态投影：发起请求、流式开始/完成、完成状态与错误分类。

use super::*;

/// 取本次渠道尝试的首个 provider turn 序号；无投影通道时序号只用于内部事件身份。
pub(super) fn begin_provider_turn_epoch(progress: Option<&RunProgressSink<'_>>) -> u64 {
    progress.map_or(1, RunProgressSink::begin_provider_attempt)
}

/// 收尾投影：用量帧带上本次请求的上下文构成。
///
/// 构成是本地估算、用量是 Provider 实测：两者同帧但口径分明，界面分开展示。
pub(super) fn emit_streaming_completed(
    plan: &ProviderTransportPlan,
    progress: Option<&RunProgressSink<'_>>,
    attempt_index: usize,
    outcome: &ProviderStreamingTransportOutcome,
    composition: Option<axon_agent_runtime::ContextTokenComposition>,
) {
    if let ProviderStreamingTransportOutcome::VisibleOutput(inner) = outcome {
        emit_streaming_completed(plan, progress, attempt_index, inner, composition);
        return;
    }
    let Some(sink) = progress else {
        return;
    };
    if let ProviderStreamingTransportOutcome::AggregatedStream { stream, .. } = outcome
        && let Some(usage) = stream.token_usage
        && usage.has_tokens()
    {
        let mut usage = usage;
        if let Some(composition) = composition {
            usage.set_context_composition(composition);
        }
        sink.token_usage_updated(usage);
    }
    sink.streaming_completed(
        plan.provider_name.clone(),
        plan.model_id.clone(),
        attempt_index,
    );
}

pub(super) fn emit_streaming_started(
    plan: &ProviderTransportPlan,
    effective_transport: axon_agent_runtime::ProviderStreamingWireTransport,
    progress: Option<&RunProgressSink<'_>>,
    attempt_index: usize,
) {
    if let Some(sink) = progress {
        sink.streaming_started(
            plan.provider_name.clone(),
            plan.model_id.clone(),
            attempt_index,
        );
        if attempt_index > 1 {
            sink.provider_status(provider_status(
                plan,
                effective_transport,
                WireProviderStatusPhase::Retrying,
                attempt_index,
            ));
            sink.lifecycle_status(
                RunLifecycleStatusView::new(
                    LIFECYCLE_TITLE_RUNNING,
                    WIRE_LIFECYCLE_KIND_PROVIDER_RETRY,
                    LIFECYCLE_SUMMARY_PROVIDER_RETRY,
                    LIFECYCLE_PHASE_STREAMING,
                    Some(attempt_index),
                )
                .with_tool_name(plan.model_id.clone()),
            );
        }
    }
}

pub(super) fn send_provider_transport_request(
    request: &RunRequest,
    plan: &ProviderTransportPlan,
    transport_session: &AgentRunProviderTransportSession,
    transport_request: &ProviderStreamingTransportRequest,
    progress: Option<&RunProgressSink<'_>>,
) -> ProviderStreamingTransportOutcome {
    axon_logger::debug(&format!(
        "send_provider_transport_request: wire_transport={:?}, websocket_url={:?}, timeout={:?}",
        plan.wire_transport, plan.websocket_url, plan.timeout
    ));

    let mut normalizer = RunProgressStreamNormalizer::new(
        plan.stream_protocol,
        transport_request.turn_id.0,
        request.cwd.clone(),
    );
    let connected = Cell::new(false);
    let generating = Cell::new(false);
    let provider_started_at_ms = crate::run_commands::run_execution::now_ms();
    let result = {
        let mut on_sse_line = |line: &str| {
            if let Some(sink) = progress {
                let effective_transport = transport_session.effective_transport();
                if !connected.replace(true) {
                    sink.provider_status(provider_status(
                        plan,
                        effective_transport,
                        WireProviderStatusPhase::Connected,
                        transport_request.attempt_index,
                    ));
                }
                if sink.consume_line(&mut normalizer, line) {
                    observe_visible_output(
                        request,
                        plan,
                        transport_session,
                        sink,
                        transport_request.attempt_index,
                        provider_started_at_ms,
                        &generating,
                    );
                }
            }
        };

        axon_logger::debug("调用 agent_run_send_provider_request...");
        if let Some(performance) = &request.performance {
            performance.record(TurnPerformanceStage::ProviderRequestStarted);
        }
        agent_run_send_provider_request(AgentRunProviderRequestInput {
            wire_transport: plan.wire_transport,
            logical_session: &plan.logical_session,
            session: transport_session,
            websocket_url: plan.websocket_url.as_deref(),
            enable_request_compression: plan.enable_request_compression,
            transport_request,
            timeout: plan.timeout,
            stream_frame_callback: progress.map(|_| &mut on_sse_line as &mut dyn FnMut(&str)),
            stream_progress_callback: None,
            stream_projection_callback: None,
            cancel_token: request.cancel_token.as_ref(),
        })
    };
    if let Some(sink) = progress
        && normalizer.finish(sink)
    {
        observe_visible_output(
            request,
            plan,
            transport_session,
            sink,
            transport_request.attempt_index,
            provider_started_at_ms,
            &generating,
        );
    }
    emit_transport_completion_status(
        plan,
        progress,
        transport_request.attempt_index,
        result.effective_transport,
        &result.outcome,
        &connected,
        &generating,
    );
    log_transport_result(&result);
    record_channel_connection_observation(
        &request.config_path,
        request.balancer.telemetry_persistence_enabled,
        axon_agent_runtime::channel_metrics_key(&request.routing_key, &plan.provider_name),
        result.connection_reused,
        result.connection_age_ms,
        result.fallback_activated,
        crate::run_commands::run_execution::now_ms(),
    );

    if generating.get() {
        result.outcome.with_visible_output()
    } else {
        result.outcome
    }
}

pub(super) fn emit_transport_completion_status(
    plan: &ProviderTransportPlan,
    progress: Option<&RunProgressSink<'_>>,
    attempt_index: usize,
    effective_transport: axon_agent_runtime::ProviderStreamingWireTransport,
    outcome: &ProviderStreamingTransportOutcome,
    connected: &Cell<bool>,
    generating: &Cell<bool>,
) {
    if !matches!(
        outcome,
        ProviderStreamingTransportOutcome::AggregatedStream { .. }
    ) {
        return;
    }
    let Some(sink) = progress else {
        return;
    };
    if !connected.replace(true) {
        sink.provider_status(provider_status(
            plan,
            effective_transport,
            WireProviderStatusPhase::Connected,
            attempt_index,
        ));
    }
    if !generating.replace(true) {
        sink.provider_status(provider_status(
            plan,
            effective_transport,
            WireProviderStatusPhase::Generating,
            attempt_index,
        ));
    }
}

/// 取本次尝试的 Provider 实测用量；错误尝试与无用量帧返回 `None`。
pub(super) fn transport_outcome_token_usage(
    outcome: &ProviderStreamingTransportOutcome,
) -> Option<axon_agent_runtime::TokenUsageTelemetry> {
    match outcome {
        ProviderStreamingTransportOutcome::AggregatedStream { stream, .. } => stream.token_usage,
        ProviderStreamingTransportOutcome::VisibleOutput(inner) => {
            transport_outcome_token_usage(inner)
        }
        ProviderStreamingTransportOutcome::Error(_)
        | ProviderStreamingTransportOutcome::StreamCompleted { .. } => None,
    }
}

/// 本次尝试是否拿到可用响应：只有 `Error` 算失败，缺用量帧不等于失败。
pub(super) fn transport_outcome_succeeded(outcome: &ProviderStreamingTransportOutcome) -> bool {
    match outcome {
        ProviderStreamingTransportOutcome::Error(_) => false,
        ProviderStreamingTransportOutcome::VisibleOutput(inner) => {
            transport_outcome_succeeded(inner)
        }
        ProviderStreamingTransportOutcome::AggregatedStream { .. }
        | ProviderStreamingTransportOutcome::StreamCompleted { .. } => true,
    }
}

/// 登记本次请求的 prompt cache 形状与实测命中。
///
/// 样本恒落盘，日志行由遥测模块按断裂严重度分流：正常追加走 DEBUG，真实断裂走 WARN。
pub(super) fn record_prompt_cache_observation(
    request: &RunRequest,
    plan: &ProviderTransportPlan,
    transport_request: &ProviderStreamingTransportRequest,
    outcome: &ProviderStreamingTransportOutcome,
) {
    crate::runtime_prompt_cache_metrics::record(
        &request.config_path,
        &request.session_id,
        &plan.provider_name,
        &plan.model_id,
        plan.stream_protocol,
        transport_request.attempt_index,
        &transport_request.cache_shape,
        transport_outcome_succeeded(outcome),
        transport_outcome_token_usage(outcome).as_ref(),
    );
}

pub(super) fn log_transport_result(
    result: &axon_provider_transport::AgentRunProviderTransportResult,
) {
    match &result.outcome {
        ProviderStreamingTransportOutcome::AggregatedStream {
            http_status,
            stream,
        } => {
            axon_logger::debug(&format!(
                "agent_run_send_provider_request 返回: AggregatedStream, http_status={http_status}, provider_event_count={}, effective_transport={:?}, fallback_activated={}, connection_reused={:?}, connection_age_ms={:?}, request_id={:?}, elapsed_ms={}",
                stream.provider_event_count,
                result.effective_transport,
                result.fallback_activated,
                result.connection_reused,
                result.connection_age_ms,
                result.request_id,
                result.elapsed_ms
            ));
        }
        ProviderStreamingTransportOutcome::Error(err) => {
            axon_logger::error(&format!(
                "agent_run_send_provider_request 返回 Error: message={}, http_status={:?}, failure_kind={:?}, provider_event_type={:?}, request_id={:?}, retry_after_ms={:?}, effective_transport={:?}, fallback_activated={}, connection_reused={:?}, connection_age_ms={:?}, elapsed_ms={}",
                err.message,
                err.http_status,
                err.failure_kind,
                err.provider_event_type,
                err.request_id,
                err.retry_after_ms,
                result.effective_transport,
                result.fallback_activated,
                result.connection_reused,
                result.connection_age_ms,
                result.elapsed_ms
            ));
        }
        ProviderStreamingTransportOutcome::StreamCompleted { .. } => {
            unreachable!("transport 完成信号必须在 attempt 边界聚合")
        }
        ProviderStreamingTransportOutcome::VisibleOutput(_) => {
            unreachable!("可见输出标记只能在 transport 结果记录后附加")
        }
    }
}

pub(super) fn provider_status(
    plan: &ProviderTransportPlan,
    effective_transport: axon_agent_runtime::ProviderStreamingWireTransport,
    phase: WireProviderStatusPhase,
    attempt_index: usize,
) -> WireProviderStatus {
    WireProviderStatus {
        provider: plan.provider_name.clone(),
        model: plan.model_id.clone(),
        phase,
        transport: match effective_transport {
            axon_agent_runtime::ProviderStreamingWireTransport::Http => WireProviderTransport::Http,
            axon_agent_runtime::ProviderStreamingWireTransport::WebSocket => {
                WireProviderTransport::WebSocket
            }
        },
        attempt_index: u64::try_from(attempt_index).unwrap_or(u64::MAX),
        http_status: None,
        error_message: None,
        next_provider: None,
        next_model: None,
    }
}

pub(super) fn interrupted_transport_outcome() -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
        "任务已由用户中断",
        None,
    ))
}
