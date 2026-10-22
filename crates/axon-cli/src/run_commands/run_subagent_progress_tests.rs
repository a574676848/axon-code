use axon_protocol::wire::{
    WireCorrelationId, WireEventEnvelope, WireEventKind, WireProviderStatus,
    WireProviderStatusPhase, WireProviderTransport, WireSchemaVersion, WireSessionRef,
    WireUsageUpdate,
};

use super::*;

#[test]
fn subagent_terminal_status_separates_interruption_from_failure() {
    // 被父轮连坐或用户中止的子代理必须记 interrupted，否则界面只能显示「本轮执行失败」。
    assert_eq!(
        subagent_terminal_status(AGENT_RUN_ERROR_TYPE_INTERRUPTED),
        TURN_STATUS_INTERRUPTED
    );
    assert_eq!(
        subagent_terminal_status("provider_transport"),
        TURN_STATUS_FAILED
    );
    assert_eq!(subagent_terminal_status("runtime"), TURN_STATUS_FAILED);
}

#[test]
fn same_channel_next_request_stays_running_until_recovery_is_required() {
    let mut projection =
        SubagentProgressProjection::new("/root/review".to_owned(), 123, "fixture-model".to_owned());

    let initial = projection
        .project(provider_event(1, WireProviderStatusPhase::Connecting))
        .unwrap();
    assert_eq!(initial.phase, PHASE_CONNECTING);

    let connected = projection
        .project(provider_event(2, WireProviderStatusPhase::Connected))
        .unwrap();
    assert_eq!(connected.phase, PHASE_RUNNING);
    assert_eq!(
        connected.tool_name.as_deref(),
        Some("fixture-provider/fixture-model")
    );

    let usage = projection.project(usage_event(6)).unwrap();
    assert_eq!(usage.total_tokens, Some(3_700));
    assert_eq!(usage.cache_hit_rate_basis_points, Some(6_060));

    assert!(
        projection
            .project(provider_event(3, WireProviderStatusPhase::Connecting))
            .is_none()
    );
    assert_eq!(projection.last_phase.as_deref(), Some(PHASE_RUNNING));

    let failed = projection
        .project(provider_event(4, WireProviderStatusPhase::Failed))
        .unwrap();
    assert_eq!(failed.phase, PHASE_PROVIDER_FAILED);

    let reconnecting = projection
        .project(provider_event(5, WireProviderStatusPhase::Connecting))
        .unwrap();
    assert_eq!(reconnecting.phase, PHASE_CONNECTING);
}

fn usage_event(sequence: u64) -> RunProgressWireEvent {
    RunProgressWireEvent {
        event: WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("usage-event-{sequence}")),
            correlation_id: WireCorrelationId("usage".to_owned()),
            session: WireSessionRef {
                session_id: "child-session".to_owned(),
                run_id: Some("child-run".to_owned()),
                parent_session_id: Some("parent-session".to_owned()),
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap(),
            kind: WireEventKind::UsageUpdate(WireUsageUpdate {
                input_tokens: 1_000,
                output_tokens: 400,
                cached_input_tokens: 2_000,
                cache_write_input_tokens: 300,
                reasoning_output_tokens: 0,
                current_context_tokens: 3_700,
                system_prompt_tokens: 0,
                tool_definition_tokens: 0,
                conversation_tokens: 0,
            }),
        },
    }
}

fn provider_event(sequence: u64, phase: WireProviderStatusPhase) -> RunProgressWireEvent {
    RunProgressWireEvent {
        event: WireEventEnvelope {
            schema_version: WireSchemaVersion::default(),
            event_id: WireCorrelationId(format!("provider-event-{sequence}")),
            correlation_id: WireCorrelationId("provider-status".to_owned()),
            session: WireSessionRef {
                session_id: "child-session".to_owned(),
                run_id: Some("child-run".to_owned()),
                parent_session_id: Some("parent-session".to_owned()),
            },
            sequence,
            recorded_at_ms: i64::try_from(sequence).unwrap(),
            kind: WireEventKind::ProviderStatus(WireProviderStatus {
                provider: "fixture-provider".to_owned(),
                model: "fixture-model".to_owned(),
                phase,
                transport: WireProviderTransport::Http,
                attempt_index: 1,
                http_status: None,
                error_message: None,
                next_provider: None,
                next_model: None,
            }),
        },
    }
}
