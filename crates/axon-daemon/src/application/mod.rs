//! Daemon 应用服务。

mod daemon_host;
mod daemon_host_event_subscription;
pub(super) mod egress_bridge;
pub(super) mod event_stream_bridge;
mod event_stream_bridge_telemetry;
mod external_event_ingress;
mod interaction_ingress;
mod primary_turn_lanes;
mod primary_turn_service;
mod primary_turn_worker_pool;
mod scheduler_service;
mod session_registry;
mod worker_supervisor;

pub use daemon_host::{
    DaemonHost, DaemonHostError, GatewayHostDependencies, HostTickReport, PRIMARY_TURN_ACTION_KIND,
    SessionAttachSnapshot, primary_session_id_for_interaction, queued_action,
};
pub use event_stream_bridge::{
    EventStreamBridge, EventStreamSubscription, StreamEvent, StreamNotification,
};
pub use event_stream_bridge_telemetry::EventStreamBridgeTelemetrySnapshot;
pub use external_event_ingress::{
    DEFAULT_EXTERNAL_EVENT_MAX_CLOCK_SKEW_MS, ExternalEventEnvelope, ExternalEventIngress,
    ExternalEventIngressError, MAX_EXTERNAL_EVENT_CORRELATION_KEY_LENGTH,
    MAX_EXTERNAL_EVENT_ID_LENGTH, MAX_EXTERNAL_EVENT_TITLE_LENGTH,
};
pub use interaction_ingress::{
    ActiveTurnSteerSink, InteractionDelivery, InteractionIngressPolicy,
    InteractionIngressPolicyError, InteractionIngressReceipt, NoActiveTurnSteerSink,
    PrimaryTurnIngress,
};
pub use primary_turn_service::{
    PrimaryTurnErrorLayer, PrimaryTurnFailure, PrimaryTurnRequest, PrimaryTurnRunner,
    PrimaryTurnRunnerError, PrimaryTurnService, PrimaryTurnServiceError, PrimaryTurnTickReport,
};
pub use scheduler_service::{SchedulerError, SchedulerService, SchedulerTickReport};
pub use session_registry::SessionRegistry;
pub use worker_supervisor::WorkerSupervisorState;
