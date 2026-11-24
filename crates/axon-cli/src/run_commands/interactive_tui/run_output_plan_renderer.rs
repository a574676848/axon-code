use axon_protocol::wire::WirePlanUpdate;
use axon_tui_runtime::{AgentTuiPlanLine, AgentTuiPlanStep};

/// 单帧 plan 快照投影；durable 顺序由调用方保证。
pub(super) fn wire_plan_line(plan: &WirePlanUpdate) -> AgentTuiPlanLine {
    AgentTuiPlanLine::with_id(
        plan.part_id.clone(),
        plan.title.clone(),
        plan.explanation.clone(),
        plan.steps
            .iter()
            .map(|step| {
                AgentTuiPlanStep::new(
                    &step.status,
                    step.step.clone(),
                    step.cycle,
                    step.diagnostic_count,
                    step.failing_step_count,
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_protocol::wire::{WirePlanStep, WirePlanUpdate};

    fn wire_plan(status: &str) -> WirePlanUpdate {
        WirePlanUpdate {
            part_id: "plan:session-1".to_owned(),
            title: "计划".to_owned(),
            explanation: None,
            steps: vec![WirePlanStep {
                step: "验证门禁".to_owned(),
                status: status.to_owned(),
                cycle: None,
                diagnostic_count: 0,
                failing_step_count: 0,
            }],
        }
    }

    #[test]
    fn wire_plan_line_uses_payload_part_id() {
        let plan = wire_plan_line(&wire_plan("completed"));

        assert_eq!(plan.id, "plan:session-1");
        assert!(matches!(
            plan.steps[0].status,
            axon_tui_runtime::AgentTuiPlanStatus::Completed
        ));
    }
}
