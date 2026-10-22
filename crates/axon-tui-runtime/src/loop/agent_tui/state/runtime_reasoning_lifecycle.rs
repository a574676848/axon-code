//! Runtime reasoning part 生命周期。

use crate::r#loop::agent_tui::{
    run_output::{AgentTuiReasoningPart, AgentTuiTranscriptPart, render_transcript_parts},
    runtime_bridge::RuntimePartState,
    state::AgentTuiState,
};
use crate::surface::cell::{RenderCellData, RenderCellRecord};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_reasoning_delta_batch(
        &mut self,
        part_id: String,
        deltas: Vec<String>,
    ) {
        let delta = deltas
            .into_iter()
            .filter(|delta| !delta.trim().is_empty())
            .collect::<String>();
        if part_id.trim().is_empty() || delta.trim().is_empty() {
            return;
        }
        if !self.bridge.active_reasoning_parts.contains_key(&part_id) {
            // durable 补投可能缺失 Start，按 delta 自身重建 part，避免事实被吞。
            self.start_reasoning_part(part_id.clone());
        }
        let existing_text = self.active_reasoning_cell_text(&part_id);
        self.record_visible_turn_activity();
        let text = existing_text.map_or(delta.clone(), |existing| format!("{existing}{delta}"));
        self.upsert_active_reasoning_part(part_id, text, false);
    }

    pub(in crate::r#loop::agent_tui) fn start_reasoning_part(&mut self, part_id: String) {
        if part_id.trim().is_empty() || self.bridge.active_reasoning_parts.contains_key(&part_id) {
            return;
        }
        self.bridge
            .active_reasoning_started_at
            .insert(part_id.clone(), std::time::Instant::now());
        self.bridge
            .active_reasoning_parts
            .insert(part_id, RuntimePartState::Pending);
    }

    pub(in crate::r#loop::agent_tui) fn finish_reasoning_part(&mut self, part_id: String) {
        if part_id.trim().is_empty() {
            return;
        }
        let started_at = self.bridge.active_reasoning_started_at.remove(&part_id);
        let Some(state) = self.bridge.active_reasoning_parts.remove(&part_id) else {
            return;
        };
        if self.viewport.selected_runtime_part_id() == Some(part_id.as_str()) {
            self.viewport.clear_runtime_selection();
        }
        if let Some(anchor) = state.anchor() {
            let index = anchor.cell_index();
            if let Some(line) = self.cells.transcript.get_mut(index)
                && let Some(RenderCellData::Reasoning(cell)) = line.data.as_mut()
            {
                cell.done = true;
                cell.elapsed_ms = started_at.as_ref().map(reasoning_elapsed_ms);
                line.bump_render_revision();
            }
            let _ = self.cells.transcript.commit_cell(index);
        }
    }

    fn active_reasoning_cell_text(&self, part_id: &str) -> Option<String> {
        let index = self
            .bridge
            .active_reasoning_parts
            .get(part_id)?
            .anchor()?
            .cell_index();
        let existing = self.cells.transcript.get(index)?;
        let Some(RenderCellData::Reasoning(cell)) = &existing.data else {
            return None;
        };
        (cell.id == part_id).then(|| cell.text.clone())
    }

    fn upsert_active_reasoning_part(&mut self, part_id: String, text: String, done: bool) {
        if let Some(line) = reasoning_transcript_part_cell(part_id, text, done) {
            self.upsert_active_reasoning_cell(line);
        }
    }
}

pub(in crate::r#loop::agent_tui) fn reasoning_elapsed_ms(started_at: &std::time::Instant) -> u64 {
    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn reasoning_transcript_part_cell(
    part_id: String,
    text: String,
    done: bool,
) -> Option<RenderCellRecord> {
    render_transcript_parts([AgentTuiTranscriptPart::Reasoning(
        AgentTuiReasoningPart::new(part_id, text, done),
    )])
    .into_iter()
    .next()
}
