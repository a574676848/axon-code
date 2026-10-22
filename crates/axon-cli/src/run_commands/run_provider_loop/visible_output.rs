use std::cell::Cell;

use axon_protocol::wire::WireProviderStatusPhase;
use axon_provider_transport::AgentRunProviderTransportSession;

use super::ProviderTransportPlan;
use super::provider_transport_attempt::provider_status;
use crate::run_commands::{RunRequest, run_progress::RunProgressSink};

pub(super) fn observe_visible_output(
    request: &RunRequest,
    plan: &ProviderTransportPlan,
    transport_session: &AgentRunProviderTransportSession,
    sink: &RunProgressSink<'_>,
    attempt_index: usize,
    provider_started_at_ms: u64,
    generating: &Cell<bool>,
) {
    if generating.replace(true) {
        return;
    }
    axon_logger::debug(&format!(
        "Provider 首个可见输出: session_id={}, provider={}, model={}, first_visible_ms={}",
        request.session_id,
        plan.provider_name,
        plan.model_id,
        crate::run_commands::run_execution::now_ms().saturating_sub(provider_started_at_ms)
    ));
    sink.provider_status(provider_status(
        plan,
        transport_session.effective_transport(),
        WireProviderStatusPhase::Generating,
        attempt_index,
    ));
}
