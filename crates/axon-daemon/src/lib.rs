//! 通用后台 Session 宿主。
//!
//! DaemonHost 管理 ownership、Session Worker、调度和 IPC 端口；
//! 不包含 CLI、Feishu 或 Agent prompt 逻辑。

pub mod application;
pub mod infrastructure;
pub mod model;
pub mod ports;

pub use application::{
    ActiveTurnSteerSink, DEFAULT_EXTERNAL_EVENT_MAX_CLOCK_SKEW_MS, DaemonHost, DaemonHostError,
    EventStreamBridge, EventStreamBridgeTelemetrySnapshot, EventStreamSubscription,
    ExternalEventEnvelope, ExternalEventIngress, ExternalEventIngressError,
    GatewayHostDependencies, HostTickReport, InteractionDelivery, InteractionIngressPolicy,
    InteractionIngressPolicyError, InteractionIngressReceipt, NoActiveTurnSteerSink,
    PRIMARY_TURN_ACTION_KIND, PrimaryTurnErrorLayer, PrimaryTurnFailure, PrimaryTurnIngress,
    PrimaryTurnRequest, PrimaryTurnRunner, PrimaryTurnRunnerError, PrimaryTurnService,
    PrimaryTurnServiceError, PrimaryTurnTickReport, SessionAttachSnapshot, StreamEvent,
    StreamNotification, primary_session_id_for_interaction,
};
pub use infrastructure::event_scripts::{
    EVENT_SCRIPT_OUTPUT_FIELDS, EVENT_SCRIPT_SCHEMA_JSON, EVENT_SCRIPT_TIMEOUT,
    EventProviderRegistry, EventScriptError, EventScriptEvent,
};
pub use infrastructure::gateway_auth::{GatewayAuth, GatewayAuthError};
pub use infrastructure::gateway_bridge::{
    DaemonGatewayBridge, EVENT_CONTENT_INLINE_LIMIT_CHARS, GatewayEventOptions,
};
pub use infrastructure::gateway_debounce::{GATEWAY_DEBOUNCE_DEFAULT_WINDOW, GatewayDebouncer};
pub use infrastructure::gateway_http::HttpGatewayAdapter;
pub use infrastructure::gateway_http_handler::{GatewayHttpResponse, GatewayHttpRouteHandler};
pub use infrastructure::gateway_kernel::{
    GatewayAdapter, GatewayConnectionLimiter, GatewayConnectionPermit, GatewayContext,
    GatewayEgress, GatewayEvent, GatewayEventStream, GatewayIngress, GatewayKernel,
    GatewayKernelError, GatewayLifecycle, GatewayMailbox, GatewaySubscriptionRequest,
};
pub use infrastructure::gateway_websocket::WebSocketGatewayAdapter;
pub use infrastructure::log_sink::LogSink;
pub use model::{DaemonHostStatus, UpgradeAdmission};
pub use ports::{DaemonClock, ProcessIdentity};
