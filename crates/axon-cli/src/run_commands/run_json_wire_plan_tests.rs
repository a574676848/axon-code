use super::*;

#[test]
fn completed_json_includes_plan_update_from_tool_receipt() {
    let mut builder = RunWireEventBuilder {
        session: WireSessionRef {
            session_id: "session-1".to_owned(),
            run_id: None,
            parent_session_id: None,
        },
        recorded_at_ms: 1,
        next_sequence: 1,
        events: Vec::new(),
    };

    builder.push_tool_receipts(&[RunToolReceipt {
        tool_call_id: 7,
        part_id: "tool-part:plan".to_owned(),
        provider_call_id: "provider-plan-7".to_owned(),
        tool_name: "builtin.update_plan".to_owned(),
        status: "completed".to_owned(),
        summary: "计划已更新".to_owned(),
        metadata: Some(serde_json::json!({
            "schemaVersion": axon_agent_runtime::UPDATE_PLAN_TOOL_SCHEMA_VERSION,
            "kind": axon_agent_runtime::UPDATE_PLAN_RECEIPT_KIND,
            "title": "计划",
            "plan": [
                {"step": "证据闭环", "status": "completed", "verification": "已检查"},
                {"step": "实现闭环", "status": "in_progress"},
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
    }]);

    let events = builder.into_events();
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        WireEventKind::PlanUpdate(plan) if plan.part_id == "plan:session-1"
            && plan.steps[1].status == "in_progress"
    )));
}
