//! update_plan 工具回执到 wire PlanUpdate 的唯一投影。

use axon_agent_runtime::{
    UPDATE_PLAN_RECEIPT_KIND, UPDATE_PLAN_TOOL_SCHEMA_VERSION, UpdatePlanToolExecutionReceipt,
};
use axon_protocol::wire::{WirePlanStep, WirePlanUpdate};
use serde_json::Value;

const COMPLETED_TOOL_STATUS: &str = "completed";
const PLAN_PART_PREFIX: &str = "plan";

pub(super) fn plan_update_from_tool_receipt(
    session_id: &str,
    status: &str,
    metadata: Option<&Value>,
) -> Option<WirePlanUpdate> {
    if status != COMPLETED_TOOL_STATUS {
        return None;
    }
    let receipt =
        serde_json::from_value::<UpdatePlanToolExecutionReceipt>(metadata?.clone()).ok()?;
    if receipt.kind != UPDATE_PLAN_RECEIPT_KIND
        || receipt.schema_version != UPDATE_PLAN_TOOL_SCHEMA_VERSION
    {
        return None;
    }
    Some(WirePlanUpdate {
        part_id: plan_part_id(session_id),
        title: receipt.title,
        explanation: receipt.explanation,
        steps: receipt
            .plan
            .into_iter()
            .map(|step| WirePlanStep {
                step: step.step,
                status: step.status.as_str().to_owned(),
                cycle: None,
                diagnostic_count: 0,
                failing_step_count: 0,
            })
            .collect(),
    })
}

fn plan_part_id(session_id: &str) -> String {
    format!("{PLAN_PART_PREFIX}:{session_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_stable_part_id_and_step_statuses() {
        let metadata = serde_json::json!({
            "schemaVersion": UPDATE_PLAN_TOOL_SCHEMA_VERSION,
            "kind": UPDATE_PLAN_RECEIPT_KIND,
            "title": "计划",
            "explanation": "进入验证",
            "plan": [
                {"step": "实现完成", "status": "completed", "verification": "测试通过"},
                {"step": "门禁通过", "status": "in_progress"},
                {"step": "文档同步", "status": "pending"}
            ],
            "completedSteps": 1,
            "totalSteps": 3
        });

        let plan = plan_update_from_tool_receipt("session-1", "completed", Some(&metadata))
            .expect("合法回执应投影计划");

        assert_eq!(plan.part_id, "plan:session-1");
        assert_eq!(plan.explanation.as_deref(), Some("进入验证"));
        assert_eq!(plan.steps[0].status, "completed");
        assert_eq!(plan.steps[1].status, "in_progress");
    }

    #[test]
    fn ignores_failed_or_unrelated_receipts() {
        assert!(plan_update_from_tool_receipt("session-1", "failed", None).is_none());
        assert!(
            plan_update_from_tool_receipt(
                "session-1",
                "completed",
                Some(&serde_json::json!({"kind": "search"}))
            )
            .is_none()
        );
    }
}
