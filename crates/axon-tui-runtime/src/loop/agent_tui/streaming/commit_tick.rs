//! Assistant/plan 双 controller 的 commit tick 协调器。

use std::time::{Duration, Instant};

use crate::r#loop::agent_tui::streaming::chunking::{
    AdaptiveChunkingPolicy, DrainPlan, QueueSnapshot,
};
use crate::r#loop::agent_tui::streaming::controller::{
    AssistantStreamController, PlanStreamController,
};
use crate::surface::cell::RenderCellRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitTickScope {
    AnyMode,
    CatchUpOnly,
}

#[derive(Debug, Default)]
pub(crate) struct CommitTickOutput {
    pub(crate) assistant_cell: Option<RenderCellRecord>,
    pub(crate) drained_plan_cells: Vec<RenderCellRecord>,
    pub(crate) has_controller: bool,
    pub(crate) all_idle: bool,
    pub(crate) should_stop_commit_animation: bool,
}

pub(crate) fn run_commit_tick(
    policy: &mut AdaptiveChunkingPolicy,
    assistant: Option<&mut AssistantStreamController>,
    plan: Option<&mut PlanStreamController>,
    scope: CommitTickScope,
    now: Instant,
) -> CommitTickOutput {
    let snapshot = queue_snapshot(assistant.as_deref(), plan.as_deref(), now);
    let decision = policy.decide(snapshot, now);
    if scope == CommitTickScope::CatchUpOnly
        && decision.mode == crate::r#loop::agent_tui::streaming::chunking::ChunkingMode::Smooth
    {
        return CommitTickOutput::default();
    }
    apply_drain_plan(decision.drain_plan, assistant, plan)
}

fn queue_snapshot(
    assistant: Option<&AssistantStreamController>,
    plan: Option<&PlanStreamController>,
    now: Instant,
) -> QueueSnapshot {
    let queued_lines = assistant.map_or(0, AssistantStreamController::queued_lines)
        + plan.map_or(0, PlanStreamController::queued_lines);
    let oldest_age = max_duration(
        assistant.and_then(|controller| controller.oldest_queued_age(now)),
        plan.and_then(|controller| controller.oldest_queued_age(now)),
    );
    QueueSnapshot {
        queued_lines,
        oldest_age,
    }
}

fn apply_drain_plan(
    drain_plan: DrainPlan,
    assistant: Option<&mut AssistantStreamController>,
    plan: Option<&mut PlanStreamController>,
) -> CommitTickOutput {
    let mut output = CommitTickOutput {
        all_idle: true,
        ..CommitTickOutput::default()
    };
    if let Some(controller) = assistant {
        output.has_controller = true;
        output.assistant_cell = drain_controller(drain_plan, controller)
            .or_else(|| controller.take_active_projection_cell());
        output.all_idle &= controller.queued_lines() == 0 && !controller.has_live_tail();
    }
    if let Some(controller) = plan {
        output.has_controller = true;
        output
            .drained_plan_cells
            .extend(drain_plan_controller(drain_plan, controller));
        output.all_idle &= controller.queued_lines() == 0 && !controller.has_live_tail();
    }
    output.should_stop_commit_animation = output.has_controller && output.all_idle;
    output
}

fn drain_controller(
    drain_plan: DrainPlan,
    controller: &mut AssistantStreamController,
) -> Option<RenderCellRecord> {
    match drain_plan {
        DrainPlan::Single => controller.on_commit_tick(),
        DrainPlan::Batch(max_lines) => controller.on_commit_tick_batch(max_lines),
    }
}

fn drain_plan_controller(
    drain_plan: DrainPlan,
    controller: &mut PlanStreamController,
) -> Vec<RenderCellRecord> {
    match drain_plan {
        DrainPlan::Single => controller.on_commit_tick(),
        DrainPlan::Batch(max_lines) => controller.on_commit_tick_batch(max_lines),
    }
}

fn max_duration(left: Option<Duration>, right: Option<Duration>) -> Option<Duration> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_tick_drains_one_cell_across_controllers() {
        let now = Instant::now();
        let mut policy = AdaptiveChunkingPolicy::default();
        let mut assistant = AssistantStreamController::new("assistant:test", 80, ".");
        let mut plan = PlanStreamController::new(80, ".");
        assistant.push_delta("a\n", now);
        plan.push_delta("p\n", now);

        let output = run_commit_tick(
            &mut policy,
            Some(&mut assistant),
            Some(&mut plan),
            CommitTickScope::AnyMode,
            now,
        );

        assert!(output.assistant_cell.is_some());
        assert_eq!(output.drained_plan_cells.len(), 1);
        assert!(output.should_stop_commit_animation);
    }

    #[test]
    fn catch_up_tick_drains_backlog_batch() {
        let now = Instant::now();
        let mut policy = AdaptiveChunkingPolicy::default();
        let mut assistant = AssistantStreamController::new("assistant:test", 80, ".");
        assistant.push_delta("1\n2\n3\n4\n5\n6\n7\n8\n", now);

        let output = run_commit_tick(
            &mut policy,
            Some(&mut assistant),
            None,
            CommitTickScope::AnyMode,
            now + Duration::from_millis(130),
        );

        assert_eq!(
            output.assistant_cell.expect("assistant cell").text,
            "1\n2\n3\n4\n5\n6\n7\n8\n"
        );
        assert!(output.should_stop_commit_animation);
    }

    #[test]
    fn tick_projects_a_held_table_once_per_source_revision() {
        let now = Instant::now();
        let mut policy = AdaptiveChunkingPolicy::default();
        let mut assistant = AssistantStreamController::new("assistant:test", 80, ".");
        assistant.push_delta("| 名称 | 状态 |\n", now);

        let first = run_commit_tick(
            &mut policy,
            Some(&mut assistant),
            None,
            CommitTickScope::AnyMode,
            now,
        );
        assert_eq!(
            first.assistant_cell.expect("held table source").text,
            "| 名称 | 状态 |\n"
        );

        let second = run_commit_tick(
            &mut policy,
            Some(&mut assistant),
            None,
            CommitTickScope::AnyMode,
            now + Duration::from_millis(1),
        );
        assert!(second.assistant_cell.is_none());

        assistant.push_delta("| partial", now + Duration::from_millis(2));
        let hidden_tail = run_commit_tick(
            &mut policy,
            Some(&mut assistant),
            None,
            CommitTickScope::AnyMode,
            now + Duration::from_millis(2),
        );
        assert!(hidden_tail.assistant_cell.is_none());
    }
}
