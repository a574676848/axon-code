//! Provider 真实网络传输 adapter。
//!
//! `axon-agent-runtime` 只保留 provider request、runner 和 transport outcome 合同；
//! 本 crate 负责 HTTP 与 WebSocket 等外部传输细节。
pub use axon_agent_runtime::{
    ProviderStreamingHttpRequestPlan, ProviderStreamingTransportError,
    ProviderStreamingTransportOutcome, ProviderStreamingTransportRequest,
    ProviderStreamingWireTransport,
};

#[path = "transport/agent_run_transport.rs"]
mod agent_run_transport;
#[path = "transport/provider_transport.rs"]
mod provider_transport;
#[path = "transport/provider_transport_retry.rs"]
mod provider_transport_retry;
mod runtime_limits;

pub use agent_run_transport::{
    AgentRunProviderRequestInput, AgentRunProviderTransportResult,
    AgentRunProviderTransportSession, AgentRunStreamFrame, agent_run_provider_transport_error,
    agent_run_send_provider_request, mark_websocket_endpoint_fallback,
};
pub use provider_transport::{
    PROVIDER_HTTP_TRANSPORT_REASON_CANCELLED, PROVIDER_HTTP_TRANSPORT_REASON_REMOTE_CLOSED,
    PROVIDER_WEBSOCKET_TRANSPORT_OK_STATUS, PROVIDER_WEBSOCKET_TRANSPORT_REASON_REMOTE_CLOSED,
    ProviderHttpTransportInput, ProviderHttpTransportRuntime, ProviderWebSocketConnectionTelemetry,
    ProviderWebSocketTransportInput, ProviderWebSocketTransportRuntime,
};
pub use runtime_limits::PROVIDER_STREAM_IDLE_TIMEOUT_MS;
