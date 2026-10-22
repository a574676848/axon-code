//! Runtime payload 分发注册表。

use crate::r#loop::agent_tui::{run_output::AgentTuiRuntimePayload, state::AgentTuiState};
use crate::surface::runtime_payload::{RuntimePayload, RuntimePayloadKind};

use super::runtime_wire_projection::dispatch_wire_event;

type RuntimePayloadHandler = fn(&mut AgentTuiState, AgentTuiRuntimePayload);

#[derive(Clone, Copy)]
struct RuntimePayloadDispatchEntry {
    kind: RuntimePayloadKind,
    handler: RuntimePayloadHandler,
}

const RUNTIME_PAYLOAD_DISPATCH: &[RuntimePayloadDispatchEntry] = &[
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_BATCHED_ASSISTANT_DELTA,
        handler: dispatch_batched_assistant_delta,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_REASONING_PART_STARTED,
        handler: dispatch_reasoning_part_started,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_BATCHED_REASONING_DELTA,
        handler: dispatch_batched_reasoning_delta,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_REASONING_PART_ENDED,
        handler: dispatch_reasoning_part_ended,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_WIRE_EVENT,
        handler: dispatch_runtime_wire_event,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_WIRE_RESUME_SNAPSHOT,
        handler: dispatch_wire_resume_snapshot,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_WIRE_RUNTIME_ERROR,
        handler: dispatch_wire_runtime_error,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_WATCHER,
        handler: dispatch_watcher,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_CANCEL_REQUESTED,
        handler: dispatch_control_payload,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_INTERRUPT_REQUESTED,
        handler: dispatch_control_payload,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_SUBSCRIPTION_PHASE,
        handler: dispatch_subscription_phase,
    },
    RuntimePayloadDispatchEntry {
        kind: AgentTuiRuntimePayload::KIND_OBSERVATION,
        handler: dispatch_observation,
    },
];

pub(super) fn dispatch_runtime_payload(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let kind = payload.payload_kind();
    let Some(entry) = RUNTIME_PAYLOAD_DISPATCH
        .iter()
        .find(|entry| entry.kind == kind)
    else {
        debug_assert!(false, "未注册 runtime payload kind: {}", kind.as_str());
        return;
    };
    (entry.handler)(state, payload);
}

fn dispatch_batched_assistant_delta(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::BatchedAssistantDelta { part_id, deltas } = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_assistant_delta_batch(part_id, deltas);
}

fn dispatch_reasoning_part_started(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::ReasoningPartStarted { part_id } = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.start_reasoning_part(part_id);
}

fn dispatch_batched_reasoning_delta(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::BatchedReasoningDelta { part_id, deltas } = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_reasoning_delta_batch(part_id, deltas);
}

fn dispatch_reasoning_part_ended(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::ReasoningPartEnded { part_id } = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.finish_reasoning_part(part_id);
}

fn dispatch_wire_resume_snapshot(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::WireResumeSnapshot(snapshot) = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_wire_resume_snapshot(*snapshot);
}

fn dispatch_runtime_wire_event(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::WireEvent { event } = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    dispatch_wire_event(state, *event);
}

fn dispatch_wire_runtime_error(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::WireRuntimeError(error) = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_resume_runtime_error(error);
}

fn dispatch_watcher(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::Watcher(event) = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_watcher_event(event);
}

fn dispatch_control_payload(_state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    debug_assert!(
        matches!(
            payload,
            AgentTuiRuntimePayload::CancelRequested(_)
                | AgentTuiRuntimePayload::InterruptRequested(_)
        ),
        "runtime payload handler 与 kind 不匹配"
    );
}

fn dispatch_subscription_phase(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::SubscriptionPhase(phase) = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_subscription_phase(phase);
}

fn dispatch_observation(state: &mut AgentTuiState, payload: AgentTuiRuntimePayload) {
    let AgentTuiRuntimePayload::Observation(frame) = payload else {
        debug_assert!(false, "runtime payload handler 与 kind 不匹配");
        return;
    };
    state.apply_observation(&frame);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_payload_dispatch_registry_covers_all_declared_kinds() {
        let dispatch_kinds = RUNTIME_PAYLOAD_DISPATCH
            .iter()
            .map(|entry| entry.kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let declared_kinds = AgentTuiRuntimePayload::ALL_KINDS
            .iter()
            .map(|kind| kind.as_str())
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(dispatch_kinds, declared_kinds);
        assert_eq!(RUNTIME_PAYLOAD_DISPATCH.len(), declared_kinds.len());
    }
}
