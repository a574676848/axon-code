use super::{AgentTurnEvent, AgentTurnEventKind, RunToolReceipt};

pub(super) fn tool_lifecycle_turn_events(
    report: &axon_agent_runtime::AgentTurnRuntimeReport<RunToolReceipt>,
) -> Vec<AgentTurnEvent> {
    let mut enriched = Vec::new();
    let mut receipts = report.tool_receipts.iter();
    for event in &report.turn_events {
        if event.kind == AgentTurnEventKind::ToolResultsRecorded
            && let Some(turn_report) = report
                .turn_reports
                .iter()
                .find(|turn_report| turn_report.turn_id == event.turn_id)
        {
            for tool_call in &turn_report.transport_report.runner_report.tool_calls {
                let Some(receipt) = receipts.next() else {
                    break;
                };
                let completed_at_ms = event.recorded_at_ms;
                let started_at_ms =
                    completed_at_ms.saturating_sub(receipt.duration_ms.unwrap_or_default());
                enriched.push(tool_started_event(
                    event.turn_id,
                    tool_call,
                    receipt,
                    started_at_ms,
                ));
                enriched.push(tool_completed_event(
                    event.turn_id,
                    receipt,
                    completed_at_ms,
                ));
            }
        }
        enriched.push(event.clone());
    }
    enriched
}

fn tool_started_event(
    turn_id: axon_domain::TurnId,
    tool_call: &axon_agent_runtime::ProviderToolCall,
    receipt: &RunToolReceipt,
    recorded_at_ms: u64,
) -> AgentTurnEvent {
    let mut event = AgentTurnEvent::new(
        turn_id,
        AgentTurnEventKind::ToolExecutionStarted,
        format!("工具开始：{}", tool_call.dispatch_name()),
    );
    event.recorded_at_ms = recorded_at_ms;
    event
        .with_part_id(receipt.part_id.clone())
        .with_tool_name(tool_call.dispatch_name())
        .with_provider_call_id(tool_call.id.clone())
        .with_status("running")
        .with_phase("tool")
        .with_metadata(serde_json::json!({
            "input": tool_call.input,
        }))
}

fn tool_completed_event(
    turn_id: axon_domain::TurnId,
    receipt: &RunToolReceipt,
    recorded_at_ms: u64,
) -> AgentTurnEvent {
    let mut event = AgentTurnEvent::new(
        turn_id,
        AgentTurnEventKind::ToolCallCompleted,
        receipt.summary.clone(),
    )
    .with_part_id(receipt.part_id.clone())
    .with_tool_name(receipt.tool_name.clone())
    .with_provider_call_id(receipt.provider_call_id.clone())
    .with_status(receipt.status.clone())
    .with_phase("tool");
    event.recorded_at_ms = recorded_at_ms;
    if let Some(metadata) = receipt.metadata.clone() {
        event = event.with_metadata(metadata);
    }
    if let Some(duration_ms) = receipt.duration_ms {
        event = event.with_duration_ms(duration_ms);
    }
    event
}
