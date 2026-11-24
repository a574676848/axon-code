//! Source-backed assistant transcript consolidation.

use std::ops::Range;

use crate::{
    r#loop::agent_tui::{state::AgentTuiState, tui_event::AgentTuiEvent},
    surface::{
        cell::{RenderCellData, RenderCellRecord, RenderMessageSourceKind},
        cell_plan_data::AgentRenderPlanSourceKind,
    },
};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_tui_event(&mut self, event: AgentTuiEvent) {
        match event {
            AgentTuiEvent::ConsolidateAgentMessage { cell } => self.consolidate_agent_message(cell),
            AgentTuiEvent::ConsolidateProposedPlan { deferred_cell } => {
                self.consolidate_proposed_plan(deferred_cell)
            }
        }
    }

    fn consolidate_agent_message(&mut self, cell: RenderCellRecord) {
        let Some(part_id) = cell.semantic_part_id() else {
            return;
        };
        if cell.text.trim().is_empty() {
            self.bridge.assistant_streams.take_anchor(&part_id);
            return;
        }
        if let Some(anchor) = self.bridge.assistant_streams.take_anchor(&part_id)
            && self.cells.transcript.get(anchor.cell_index()).is_some()
        {
            let index = anchor.cell_index();
            self.cells
                .transcript
                .replace_cell(index, cell)
                .expect("active assistant anchor must reference a render record");
            self.cells
                .transcript
                .commit_cell(index)
                .expect("active assistant anchor must reference an active record");
            return;
        }
        let records = self.cells.transcript.to_record_vec();
        if let Some(range) = assistant_stream_run_for_part(&records, &part_id) {
            self.replace_transcript_range_with_line(range, cell);
            return;
        }
        if let Some(index) = self.transcript_index_by_part_id(&part_id)
            && self
                .cells
                .transcript
                .replace_cell(index, cell.clone())
                .is_ok()
        {
            let _ = self.cells.transcript.commit_cell(index);
            return;
        }
        self.cells.transcript.push(cell);
    }

    fn consolidate_proposed_plan(&mut self, deferred_cell: Option<RenderCellRecord>) {
        if let Some(cell) = deferred_cell {
            self.cells.transcript.push(cell);
        }
        let records = self.cells.transcript.to_record_vec();
        let end = records.len();
        let start = trailing_plan_stream_run_start(&records);
        if start == end {
            return;
        }
        let Some(consolidated) = consolidated_plan_line_from_stream_tail(&records[start..end])
        else {
            return;
        };
        self.replace_transcript_range_with_consolidated_plan(start..end, consolidated);
    }

    fn replace_transcript_range_with_consolidated_plan(
        &mut self,
        range: Range<usize>,
        line: RenderCellRecord,
    ) {
        self.replace_transcript_range_with_line(range, line);
    }

    fn replace_transcript_range_with_line(&mut self, range: Range<usize>, line: RenderCellRecord) {
        self.cells.transcript.splice_one(range.clone(), line);
        let removed_count = range.end.saturating_sub(range.start);
        if removed_count > 1 {
            for removed_index in (range.start + 1..range.end).rev() {
                self.cells
                    .active_runtime_cells
                    .remove_transcript_index(removed_index);
                self.cells
                    .transcript_surfaces
                    .remove_transcript_index(removed_index);
            }
        }
    }
}

fn assistant_stream_run_for_part(
    lines: &[RenderCellRecord],
    part_id: &str,
) -> Option<Range<usize>> {
    let end = lines
        .iter()
        .rposition(|line| is_stream_message_cell_for_part(line, part_id))?;
    let mut start = end;
    while start > 0 && is_stream_message_cell_for_part(&lines[start - 1], part_id) {
        start -= 1;
    }
    Some(start..end + 1)
}

fn is_stream_message_cell_for_part(line: &RenderCellRecord, part_id: &str) -> bool {
    matches!(
        &line.data,
        Some(RenderCellData::Message(cell))
            if cell.source_kind == RenderMessageSourceKind::Stream && cell.id == part_id
    )
}

fn trailing_plan_stream_run_start(lines: &[RenderCellRecord]) -> usize {
    let mut start = lines.len();
    while start > 0 && is_stream_plan_cell(&lines[start - 1]) {
        start -= 1;
    }
    start
}

fn is_stream_plan_cell(line: &RenderCellRecord) -> bool {
    matches!(
        &line.data,
        Some(RenderCellData::Plan(cell))
            if cell.source_kind == AgentRenderPlanSourceKind::Stream
    )
}

fn consolidated_plan_line_from_stream_tail(lines: &[RenderCellRecord]) -> Option<RenderCellRecord> {
    lines.iter().rev().find_map(|line| match &line.data {
        Some(RenderCellData::Plan(cell)) => Some(RenderCellRecord::plan_with_data(
            cell.clone().into_markdown_source_backed(),
        )),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        assistant_stream_run_for_part, consolidated_plan_line_from_stream_tail,
        trailing_plan_stream_run_start,
    };
    use crate::r#loop::agent_tui::run_output::{AgentTuiPlanLine, AgentTuiPlanStep};
    use crate::surface::cell::RenderCellData;
    use crate::surface::cell::RenderCellRecord;
    use crate::surface::cell_plan_data::{AgentRenderPlanCell, AgentRenderPlanSourceKind};

    #[test]
    fn assistant_stream_run_does_not_cross_another_part() {
        let lines = vec![
            RenderCellRecord::streaming_agent_message("assistant:a", "A1"),
            RenderCellRecord::streaming_agent_message("assistant:b", "B1"),
            RenderCellRecord::streaming_agent_message("assistant:a", "A2"),
        ];

        assert_eq!(
            assistant_stream_run_for_part(&lines, "assistant:a"),
            Some(2..3)
        );
    }

    #[test]
    fn trailing_run_finds_only_stream_plan_tail() {
        let final_plan = plan_line("final", AgentRenderPlanSourceKind::Markdown);
        let stream_plan = plan_line("stream", AgentRenderPlanSourceKind::Stream);
        let lines = vec![
            RenderCellRecord::user("prompt"),
            final_plan,
            stream_plan.clone(),
            stream_plan,
        ];

        assert_eq!(trailing_plan_stream_run_start(&lines), 2);
    }

    #[test]
    fn consolidated_plan_uses_latest_stream_source_backing() {
        let line = plan_line("stream", AgentRenderPlanSourceKind::Stream);
        let consolidated =
            consolidated_plan_line_from_stream_tail(&[line]).expect("plan should consolidate");

        assert!(matches!(
            consolidated.data,
            Some(RenderCellData::Plan(ref cell))
                if cell.source_kind == AgentRenderPlanSourceKind::Markdown
                    && cell.source.contains("- [~] stream")
        ));
    }

    fn plan_line(summary: &str, source_kind: AgentRenderPlanSourceKind) -> RenderCellRecord {
        let line = AgentTuiPlanLine::new(
            "计划",
            None,
            vec![AgentTuiPlanStep::new("running", summary, None, 0, 0)],
        );
        let cell = match source_kind {
            AgentRenderPlanSourceKind::Markdown => AgentRenderPlanCell::from_tui_line(&line),
            AgentRenderPlanSourceKind::Stream => AgentRenderPlanCell::stream_from_tui_line(&line),
        };
        RenderCellRecord::plan_with_data(cell)
    }
}
