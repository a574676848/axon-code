use super::*;
use crate::tool_orchestrator::{ToolExecutionRequest, ToolKind, ToolReceipt};
use axon_agent_runtime::ProviderToolCall;
use axon_domain::{ExecutionRequestId, ToolCallId, ToolExecutionPermission};

#[test]
fn completed_update_plan_emits_live_plan_after_tool_result() {
    let mut events = Vec::new();
    let request = request("call-plan-1", 1);
    let receipt = receipt("call-plan-1", "pending");

    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "session-1".to_owned());
        sink.tool_execution_completed(&request, &receipt, Some(5));
    }

    assert!(matches!(events[0].event.kind, WireEventKind::ToolResult(_)));
    assert!(matches!(
        &events[1].event.kind,
        WireEventKind::PlanUpdate(plan) if plan.part_id == "plan:session-1"
            && plan.steps[1].status == "pending"
    ));
}

#[test]
fn repeated_live_updates_keep_part_id_and_get_distinct_event_ids() {
    let mut events = Vec::new();
    {
        let mut emit = |event| events.push(event);
        let sink = RunProgressSink::with_session_id(&mut emit, "session-1".to_owned());
        sink.tool_execution_completed(
            &request("call-plan-1", 1),
            &receipt("call-plan-1", "pending"),
            None,
        );
        sink.tool_execution_completed(
            &request("call-plan-2", 2),
            &receipt("call-plan-2", "in_progress"),
            None,
        );
    }
    let plans = events
        .iter()
        .filter(|event| matches!(event.event.kind, WireEventKind::PlanUpdate(_)))
        .collect::<Vec<_>>();

    assert_eq!(plans.len(), 2);
    assert_ne!(plans[0].event.event_id, plans[1].event.event_id);
    assert!(plans.iter().all(|event| matches!(
        &event.event.kind,
        WireEventKind::PlanUpdate(plan) if plan.part_id == "plan:session-1"
    )));
}

fn request(provider_call_id: &str, id: u64) -> ToolExecutionRequest {
    ToolExecutionRequest {
        tool_call: ProviderToolCall::new(provider_call_id, "builtin.update_plan", "{}"),
        tool_call_id: ToolCallId(id),
        execution_request_id: ExecutionRequestId(id),
        part_id: format!("tool-part:{id}"),
        kind: ToolKind::UpdatePlan,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: ToolExecutionPermission::Allowed,
    }
}

fn receipt(provider_call_id: &str, middle_status: &str) -> ToolReceipt {
    ToolReceipt {
        tool_call_id: 1,
        part_id: "tool-part:1".to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: "builtin.update_plan".to_owned(),
        status: "completed".to_owned(),
        summary: "计划已更新".to_owned(),
        metadata: Some(serde_json::json!({
            "schemaVersion": axon_agent_runtime::UPDATE_PLAN_TOOL_SCHEMA_VERSION,
            "kind": axon_agent_runtime::UPDATE_PLAN_RECEIPT_KIND,
            "title": "计划",
            "plan": [
                {"step": "证据闭环", "status": "completed", "verification": "已检查"},
                {"step": "实现闭环", "status": middle_status},
                {"step": "门禁闭环", "status": "pending"}
            ],
            "completedSteps": 1,
            "totalSteps": 3
        })),
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: false,
        output_ref: None,
        duration_ms: None,
    }
}
