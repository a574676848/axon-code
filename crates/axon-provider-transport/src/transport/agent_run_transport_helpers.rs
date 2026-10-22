use std::time::{Duration, Instant};

use axon_domain::{AxonError, ProviderError};

use super::{
    AgentRunProviderTransportResult, AgentRunProviderTransportSession, ProviderRawExchangeCapture,
    ProviderStreamingTransportError, ProviderStreamingTransportOutcome,
    ProviderStreamingTransportRequest,
};
use crate::{ProviderStreamingWireTransport, ProviderWebSocketConnectionTelemetry};
use axon_agent_runtime::{
    ProviderStreamAccumulator, ProviderStreamProtocol, ProviderStreamingTransportFailureKind,
};

pub(super) fn aggregate_stream_outcome(
    outcome: ProviderStreamingTransportOutcome,
    accumulator: ProviderStreamAccumulator,
    raw_exchange: &mut ProviderRawExchangeCapture,
    request: &ProviderStreamingTransportRequest,
) -> ProviderStreamingTransportOutcome {
    if let ProviderStreamingTransportOutcome::Error(error) = &outcome {
        raw_exchange.log_failure(
            request,
            error.http_status,
            &error.message,
            error.provider_event_type.as_deref(),
        );
        return outcome;
    }
    let ProviderStreamingTransportOutcome::StreamCompleted { http_status } = outcome else {
        return outcome;
    };
    match accumulator.finish() {
        Ok(stream) => ProviderStreamingTransportOutcome::AggregatedStream {
            http_status,
            stream: Box::new(stream),
        },
        Err(error) => {
            let message = format!("{error:?}");
            raw_exchange.log_failure(request, Some(http_status), &message, None);
            ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new(message, Some(http_status))
                    .with_failure_kind(ProviderStreamingTransportFailureKind::Incomplete),
            )
        }
    }
}

pub(super) fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// 构造供 Agent run adapter 使用的 provider transport 错误结果。
pub fn agent_run_provider_transport_error(
    message: impl Into<String>,
    http_status: Option<u16>,
) -> ProviderStreamingTransportOutcome {
    ProviderStreamingTransportOutcome::Error(ProviderStreamingTransportError::new(
        message,
        http_status,
    ))
}

pub(super) fn provider_transport_outcome_from_axon_error(
    error: AxonError,
) -> ProviderStreamingTransportOutcome {
    match error {
        AxonError::Provider(ProviderError::Transport {
            message,
            http_status,
            provider_event_type,
            upstream_request_id,
        }) => ProviderStreamingTransportOutcome::Error(
            ProviderStreamingTransportError::new(message, http_status)
                .with_provider_event_type(provider_event_type)
                .with_request_id(upstream_request_id),
        ),

        error => agent_run_provider_transport_error(format!("{error:?}"), None),
    }
}

pub(super) fn observe_transport_outcome(
    session: &AgentRunProviderTransportSession,
    transport: ProviderStreamingWireTransport,
    outcome: &ProviderStreamingTransportOutcome,
) {
    match outcome {
        ProviderStreamingTransportOutcome::AggregatedStream { .. } => {
            session.record_success(transport)
        }
        ProviderStreamingTransportOutcome::Error(error) => session.record_request_failure(error),
        ProviderStreamingTransportOutcome::StreamCompleted { .. } => {
            unreachable!("transport 完成信号必须在 attempt 边界聚合")
        }
        ProviderStreamingTransportOutcome::VisibleOutput(_) => {
            unreachable!("可见输出标记只能由 CLI 投影边界附加")
        }
    }
}

pub(super) fn transport_result(
    outcome: ProviderStreamingTransportOutcome,
    effective_transport: ProviderStreamingWireTransport,
    fallback_activated: bool,
    started_at: Instant,
    connection_telemetry: Option<ProviderWebSocketConnectionTelemetry>,
) -> AgentRunProviderTransportResult {
    let request_id = match &outcome {
        ProviderStreamingTransportOutcome::Error(error) => error.request_id.clone(),
        ProviderStreamingTransportOutcome::AggregatedStream { .. } => None,
        ProviderStreamingTransportOutcome::StreamCompleted { .. }
        | ProviderStreamingTransportOutcome::VisibleOutput(_) => {
            unreachable!("transport 完成信号必须在 attempt 边界聚合")
        }
    };
    AgentRunProviderTransportResult {
        outcome,
        effective_transport,
        fallback_activated,
        elapsed_ms: duration_ms(started_at.elapsed()),
        connection_reused: connection_telemetry.map(|telemetry| telemetry.connection_reused),
        connection_age_ms: connection_telemetry.and_then(|telemetry| telemetry.connection_age_ms),
        request_id,
    }
}

pub(super) fn websocket_protocol_supported(protocol: ProviderStreamProtocol) -> bool {
    protocol == ProviderStreamProtocol::OpenAiResponses
}
