//! Runtime Plan / Review transcript cell 生命周期编排。

use crate::r#loop::agent_tui::{
    run_output::{AgentTuiPlanLine, AgentTuiReviewLine},
    state::AgentTuiState,
    tui_event::AgentTuiEvent,
};
use crate::surface::{
    cell::{RenderCellData, RenderCellRecord},
    cell_plan_data::{AgentRenderPlanCell, AgentRenderPlanSourceKind},
    cell_review_data::AgentRenderReviewCell,
};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn replace_active_plan_cell(
        &mut self,
        plan: AgentTuiPlanLine,
    ) {
        self.bridge.live_plan_line = Some(stream_plan_render_line(&plan));
    }

    pub(in crate::r#loop::agent_tui) fn upsert_active_review_cell(
        &mut self,
        review: AgentTuiReviewLine,
    ) {
        self.merge_or_replace_live_review_line(review_render_line(&review));
    }

    pub(in crate::r#loop::agent_tui) fn replace_or_append_active_plan_cell(
        &mut self,
        plan: AgentTuiPlanLine,
    ) {
        self.bridge.live_plan_line = None;
        self.apply_tui_event(AgentTuiEvent::ConsolidateProposedPlan {
            deferred_cell: Some(plan_render_line(&plan, AgentRenderPlanSourceKind::Stream)),
        });
    }

    pub(in crate::r#loop::agent_tui) fn settle_active_plan_cell(&mut self, line: RenderCellRecord) {
        self.apply_tui_event(AgentTuiEvent::ConsolidateProposedPlan {
            deferred_cell: Some(line),
        });
    }

    pub(in crate::r#loop::agent_tui) fn replace_or_append_active_review_cell(
        &mut self,
        review: AgentTuiReviewLine,
    ) {
        self.bridge.live_review_line = None;
        self.cells.transcript.push(review_render_line(&review));
    }

    pub(in crate::r#loop::agent_tui) fn settle_active_review_with_final(
        &mut self,
        final_review: Option<&AgentTuiReviewLine>,
    ) -> bool {
        let Some(active_review) = self.take_live_review_line() else {
            return false;
        };
        let settled = final_review.map_or(active_review.clone(), |review| {
            let mut final_line = review_render_line(review);
            merge_active_review_details(&mut final_line, &active_review);
            final_line
        });
        self.cells.transcript.push(settled);
        true
    }
}

fn stream_plan_render_line(plan: &AgentTuiPlanLine) -> RenderCellRecord {
    plan_render_line(plan, AgentRenderPlanSourceKind::Stream)
}

fn plan_render_line(
    plan: &AgentTuiPlanLine,
    source_kind: AgentRenderPlanSourceKind,
) -> RenderCellRecord {
    let cell = match source_kind {
        AgentRenderPlanSourceKind::Markdown => AgentRenderPlanCell::from_tui_line(plan),
        AgentRenderPlanSourceKind::Stream => AgentRenderPlanCell::stream_from_tui_line(plan),
    };
    RenderCellRecord::plan_with_data(cell)
}

fn review_render_line(review: &AgentTuiReviewLine) -> RenderCellRecord {
    RenderCellRecord::review_with_data(AgentRenderReviewCell::from_tui_line(review))
}

fn merge_active_review_details(final_line: &mut RenderCellRecord, active_line: &RenderCellRecord) {
    let (Some(RenderCellData::Review(final_cell)), Some(RenderCellData::Review(active_cell))) =
        (&mut final_line.data, &active_line.data)
    else {
        return;
    };
    for detail in &active_cell.details {
        if !final_cell.details.contains(detail) {
            final_cell.details.push(detail.clone());
        }
    }
    final_line.bump_render_revision();
}
