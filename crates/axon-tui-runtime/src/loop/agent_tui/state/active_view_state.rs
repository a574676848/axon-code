//! 运行中 RenderViewport tail 状态，不写入 committed transcript 索引。

use std::{sync::Arc, time::Instant};

use crate::r#loop::agent_tui::{
    runtime_bridge::{RuntimePartAnchor, RuntimePartState},
    state::{AgentTuiState, runtime_projection_policy::RuntimeProjectionPolicy},
    tui_event::AgentTuiEvent,
};
use crate::surface::{
    cell::{RenderCell, RenderCellData, RenderCellRecord},
    cell_event_data::AgentRenderLifecycleStatus,
    height_index::LiveTranscriptTailKey,
};
use crate::widgets::HyperlinkLine;

#[cfg(test)]
use crate::surface::cell::RenderCellMode;

const MAX_LIVE_LIFECYCLE_ITEMS: usize = 50;
const LIVE_TAIL_REVISION_FACTOR: u64 = 31;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn push_live_assistant_delta(
        &mut self,
        part_id: &str,
        delta: &str,
        now: Instant,
    ) {
        let width = self.viewport.frame_size.width.max(1);
        let cwd = std::path::PathBuf::from(&self.header.cwd);
        let evicted = self
            .bridge
            .assistant_streams
            .push_delta(part_id, delta, width, &cwd, now);
        if let Some(cell) = evicted {
            self.apply_tui_event(AgentTuiEvent::ConsolidateAgentMessage { cell });
        }
    }

    pub(in crate::r#loop::agent_tui) fn flush_live_assistant_output(&mut self) -> bool {
        let cells = self.bridge.assistant_streams.finalize_all();
        let flushed = !cells.is_empty();
        for cell in cells {
            self.apply_tui_event(AgentTuiEvent::ConsolidateAgentMessage { cell });
        }
        flushed
    }

    pub(in crate::r#loop::agent_tui) fn commit_assistant_message_snapshot(
        &mut self,
        part_id: String,
        content: String,
    ) {
        self.bridge.assistant_streams.discard_controller(&part_id);
        let cell = RenderCellRecord::agent_message(part_id.clone(), content);
        let existing_index = self
            .bridge
            .assistant_streams
            .take_anchor(&part_id)
            .map(|anchor| anchor.cell_index())
            .or_else(|| self.transcript_index_by_part_id(&part_id));
        if let Some(index) = existing_index
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

    pub(in crate::r#loop::agent_tui) fn commit_active_reasoning_cell(&mut self) {
        let parts = std::mem::take(&mut self.bridge.active_reasoning_parts);
        let started_at = std::mem::take(&mut self.bridge.active_reasoning_started_at);
        for (part_id, state) in parts {
            if self.viewport.selected_runtime_part_id() == Some(part_id.as_str()) {
                self.viewport.clear_runtime_selection();
            }
            let Some(anchor) = state.anchor() else {
                continue;
            };
            let index = anchor.cell_index();
            if let Some(line) = self.cells.transcript.get_mut(index)
                && let Some(RenderCellData::Reasoning(cell)) = line.data.as_mut()
            {
                cell.done = true;
                cell.elapsed_ms = started_at
                    .get(&part_id)
                    .map(super::runtime_reasoning_lifecycle::reasoning_elapsed_ms);
                line.bump_render_revision();
            }
            let _ = self.cells.transcript.commit_cell(index);
        }
    }

    pub(in crate::r#loop::agent_tui) fn active_transcript_lines(&self) -> Vec<&RenderCellRecord> {
        [
            self.bridge.live_plan_line.as_ref(),
            self.bridge.live_review_line.as_ref(),
            self.bridge.live_lifecycle_line.as_ref(),
            self.bridge.live_hook_line.as_ref(),
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    #[cfg(test)]
    pub(in crate::r#loop::agent_tui) fn active_view_lines(&self) -> Vec<&RenderCellRecord> {
        let mut lines = self.startup_view_lines();
        lines.extend(self.projected_active_runtime_transcript_lines());
        lines.extend(self.active_transcript_lines());
        lines
    }

    pub(in crate::r#loop::agent_tui) fn prepare_cold_bottom_mount(&mut self) -> bool {
        let runtime_line_count = self.active_runtime_transcript_lines().len();
        self.viewport.prepare_cold_bottom_mount(runtime_line_count)
    }

    pub(in crate::r#loop::agent_tui) fn complete_cold_bottom_mount_frame(&mut self) -> bool {
        self.viewport.complete_cold_bottom_mount_frame()
    }

    pub(in crate::r#loop::agent_tui) fn live_transcript_tail_lines(
        &self,
    ) -> Option<(Vec<RenderCellRecord>, LiveTranscriptTailKey)> {
        let lines = self
            .active_transcript_lines()
            .into_iter()
            .filter(|line| RuntimeProjectionPolicy::active_tail_allows(line))
            .cloned()
            .collect::<Vec<_>>();
        if lines.is_empty() {
            return None;
        }
        let revision = lines.iter().fold(0u64, |revision, line| {
            revision
                .wrapping_mul(LIVE_TAIL_REVISION_FACTOR)
                .wrapping_add(line.render_revision())
                .wrapping_add(line.transcript_key())
        });
        let transcript_animation_tick = lines
            .iter()
            .filter_map(RenderCellRecord::transcript_animation_tick)
            .max();
        Some((
            lines,
            LiveTranscriptTailKey {
                revision,
                is_stream_continuation: false,
                transcript_animation_tick,
            },
        ))
    }

    pub(in crate::r#loop::agent_tui) fn has_live_assistant_stream(&self) -> bool {
        self.bridge.assistant_streams.has_live_output()
    }

    pub(in crate::r#loop::agent_tui) fn has_live_lifecycle_line(&self) -> bool {
        self.bridge.live_lifecycle_line.is_some()
    }

    pub(in crate::r#loop::agent_tui) fn sync_stream_width(&mut self, width: u16, now: Instant) {
        self.bridge.assistant_streams.set_width(width, now);
        if let Some(controller) = self.bridge.plan_stream_controller.as_mut() {
            controller.set_width(width, now);
        }
    }

    fn active_runtime_transcript_lines(&self) -> Vec<&RenderCellRecord> {
        self.cells
            .active_runtime_cells
            .active_indices()
            .into_iter()
            .filter_map(|index| self.cells.transcript.get(index))
            .collect()
    }

    #[cfg(test)]
    fn projected_active_runtime_transcript_lines(&self) -> Vec<&RenderCellRecord> {
        let lines = self.active_runtime_transcript_lines();
        let start = self
            .viewport
            .active_runtime_projection_start(lines.len())
            .min(lines.len());
        lines.into_iter().skip(start).collect()
    }

    #[cfg(test)]
    fn startup_view_lines(&self) -> Vec<&RenderCellRecord> {
        if self.composer.submitted_prompt.is_some()
            || self.is_working()
            || !self.cells.active_runtime_cells.active_indices().is_empty()
        {
            return Vec::new();
        }
        self.cells
            .transcript
            .iter()
            .filter(|line| matches!(line.mode, RenderCellMode::Startup))
            .collect()
    }

    pub(in crate::r#loop::agent_tui) fn merge_or_replace_live_lifecycle_line(
        &mut self,
        mut line: RenderCellRecord,
    ) {
        let Some(existing) = self.bridge.live_lifecycle_line.as_mut() else {
            trim_live_lifecycle_line(&mut line);
            self.bridge.live_lifecycle_line = Some(line);
            return;
        };
        match (&mut existing.data, &line.data) {
            (
                Some(RenderCellData::GenerationStatus(existing_cell)),
                Some(RenderCellData::GenerationStatus(next)),
            ) => {
                existing.text.clone_from(&line.text);
                existing_cell.title.clone_from(&next.title);
                for item in &next.items {
                    if let Some(existing_item) = existing_cell.items.iter_mut().find(|candidate| {
                        candidate.kind == item.kind && candidate.phase == item.phase
                    }) {
                        *existing_item = item.clone();
                    } else {
                        existing_cell.items.push(item.clone());
                    }
                }
                trim_live_lifecycle_items(&mut existing_cell.items);
                existing.bump_render_revision();
            }
            (Some(RenderCellData::Status(existing_cell)), Some(RenderCellData::Status(next))) => {
                existing.text.clone_from(&line.text);
                existing_cell.title.clone_from(&next.title);
                existing_cell.items.clone_from(&next.items);
                existing.bump_render_revision();
            }
            _ => existing.replace_with(line),
        }
        if let Some(existing) = self.bridge.live_lifecycle_line.as_mut() {
            trim_live_lifecycle_line(existing);
        }
    }

    pub(in crate::r#loop::agent_tui) fn update_live_lifecycle_indicator(
        &mut self,
        indicator: String,
        animation_tick: u64,
    ) {
        let Some(line) = self.bridge.live_lifecycle_line.as_mut() else {
            return;
        };
        match line.data.as_mut() {
            Some(RenderCellData::Status(cell)) => {
                if let Some(item) = cell.items.first_mut() {
                    item.indicator = Some(indicator);
                    line.set_transcript_animation_tick(Some(animation_tick));
                    line.bump_render_revision();
                }
            }
            Some(RenderCellData::Command(cell)) => {
                if let Some(item) = cell.items.first_mut() {
                    item.indicator = Some(indicator);
                    line.set_transcript_animation_tick(Some(animation_tick));
                    line.bump_render_revision();
                }
            }
            Some(RenderCellData::GenerationStatus(cell)) => {
                if let Some(item) = cell.items.first_mut() {
                    item.indicator = Some(indicator);
                    line.set_transcript_animation_tick(Some(animation_tick));
                    line.bump_render_revision();
                }
            }
            _ => {}
        }
    }

    pub(in crate::r#loop::agent_tui) fn update_live_hook_indicator(
        &mut self,
        indicator: String,
        animation_tick: u64,
    ) {
        let Some(line) = self.bridge.live_hook_line.as_mut() else {
            return;
        };
        let Some(RenderCellData::Hook(cell)) = line.data.as_mut() else {
            return;
        };
        let mut changed = false;
        for item in &mut cell.items {
            if item.status == AgentRenderLifecycleStatus::Running {
                item.indicator = Some(indicator.clone());
                changed = true;
            }
        }
        if changed {
            line.set_transcript_animation_tick(Some(animation_tick));
            line.bump_render_revision();
        }
    }

    pub(in crate::r#loop::agent_tui) fn merge_or_replace_live_review_line(
        &mut self,
        line: RenderCellRecord,
    ) {
        let Some(existing) = self.bridge.live_review_line.as_mut() else {
            self.bridge.live_review_line = Some(line);
            return;
        };
        let (Some(RenderCellData::Review(existing_cell)), Some(RenderCellData::Review(next))) =
            (&mut existing.data, &line.data)
        else {
            existing.replace_with(line);
            return;
        };
        existing.text.clone_from(&line.text);
        existing_cell.merge_from(next.clone());
        existing.bump_render_revision();
    }

    pub(in crate::r#loop::agent_tui) fn merge_or_replace_live_hook_line(
        &mut self,
        line: RenderCellRecord,
    ) {
        let Some(existing) = self.bridge.live_hook_line.as_mut() else {
            self.bridge.live_hook_line = Some(line);
            return;
        };
        let (Some(RenderCellData::Hook(existing_cell)), Some(RenderCellData::Hook(next))) =
            (&mut existing.data, &line.data)
        else {
            existing.replace_with(line);
            return;
        };
        existing.text = "Hook".to_owned();
        existing_cell.merge_from(next.clone());
        existing.bump_render_revision();
    }

    pub(in crate::r#loop::agent_tui) fn remove_live_hook_item(&mut self, run_id: &str) {
        let Some(line) = self.bridge.live_hook_line.as_mut() else {
            return;
        };
        let Some(RenderCellData::Hook(cell)) = line.data.as_mut() else {
            self.bridge.live_hook_line = None;
            return;
        };
        cell.remove_item(run_id);
        if cell.is_empty() {
            self.bridge.live_hook_line = None;
        } else {
            line.bump_render_revision();
        }
    }

    pub(in crate::r#loop::agent_tui) fn clear_active_transcript_lines(&mut self) {
        self.commit_active_reasoning_cell();
        self.bridge.live_plan_line = None;
        self.bridge.live_review_line = None;
        self.bridge.live_lifecycle_line = None;
        self.bridge.live_hook_line = None;
        self.bridge.assistant_streams.clear();
        self.bridge.plan_stream_controller = Some(
            crate::r#loop::agent_tui::streaming::controller::PlanStreamController::new(
                self.viewport.frame_size.width.max(1),
                &self.header.cwd,
            ),
        );
        self.bridge.stream_chunking_policy.reset();
    }

    pub(in crate::r#loop::agent_tui) fn take_live_plan_line(&mut self) -> Option<RenderCellRecord> {
        self.bridge.live_plan_line.take()
    }

    pub(in crate::r#loop::agent_tui) fn take_live_review_line(
        &mut self,
    ) -> Option<RenderCellRecord> {
        self.bridge.live_review_line.take()
    }

    pub(in crate::r#loop::agent_tui) fn upsert_active_assistant_cell(
        &mut self,
        line: RenderCellRecord,
    ) {
        let Some(part_id) = assistant_cell_part_id(&line) else {
            return;
        };
        if let Some(anchor) = self.bridge.assistant_streams.anchor(&part_id).cloned() {
            let index = anchor.cell_index();
            let Some(line) = self.cells.transcript.replace_active_cell(index, line) else {
                return;
            };
            self.bridge.assistant_streams.take_anchor(&part_id);
            let index = self.cells.transcript.push_active(line);
            self.bridge
                .assistant_streams
                .set_anchor(part_id.clone(), RuntimePartAnchor::new(part_id, index));
            return;
        }
        let index = self.cells.transcript.push_active(line);
        self.bridge
            .assistant_streams
            .set_anchor(part_id.clone(), RuntimePartAnchor::new(part_id, index));
    }

    pub(in crate::r#loop::agent_tui) fn prime_active_assistant_layout(
        &self,
        part_id: &str,
        width: u16,
        lines: Arc<[HyperlinkLine]>,
    ) {
        let Some(index) = self
            .bridge
            .assistant_streams
            .anchor(part_id)
            .map(RuntimePartAnchor::cell_index)
        else {
            return;
        };
        let Some(cell) = self.cells.transcript.get(index) else {
            return;
        };
        self.viewport
            .transcript_render_cache
            .prime_streaming_layout(cell, width, lines);
    }

    pub(in crate::r#loop::agent_tui) fn upsert_active_reasoning_cell(
        &mut self,
        line: RenderCellRecord,
    ) {
        let Some(part_id) = reasoning_cell_part_id(&line) else {
            return;
        };
        if let Some(index) = self
            .bridge
            .active_reasoning_parts
            .get(&part_id)
            .and_then(RuntimePartState::anchor)
            .map(RuntimePartAnchor::cell_index)
        {
            if self
                .cells
                .transcript
                .mutate_cell(index, |existing| existing.replace_with(line.clone()))
                .is_ok()
            {
                return;
            }
            self.bridge
                .active_reasoning_parts
                .insert(part_id.clone(), RuntimePartState::Pending);
        }
        let index = self.cells.transcript.push_active(line);
        self.bridge.active_reasoning_parts.insert(
            part_id.clone(),
            RuntimePartState::Anchored(RuntimePartAnchor::new(part_id, index)),
        );
    }
}

fn reasoning_cell_part_id(line: &RenderCellRecord) -> Option<String> {
    match &line.data {
        Some(RenderCellData::Reasoning(cell)) => Some(cell.id.clone()),
        _ => None,
    }
}

fn assistant_cell_part_id(line: &RenderCellRecord) -> Option<String> {
    match &line.data {
        Some(RenderCellData::Message(cell)) => Some(cell.id.clone()),
        _ => None,
    }
}

fn trim_live_lifecycle_line(line: &mut RenderCellRecord) {
    match line.data.as_mut() {
        Some(RenderCellData::GenerationStatus(cell)) => {
            trim_live_lifecycle_items(&mut cell.items);
        }
        Some(RenderCellData::Status(cell)) => {
            trim_live_lifecycle_items(&mut cell.items);
        }
        _ => {}
    }
}

fn trim_live_lifecycle_items<T>(items: &mut Vec<T>) {
    let overflow = items.len().saturating_sub(MAX_LIVE_LIFECYCLE_ITEMS);
    if overflow > 0 {
        items.drain(..overflow);
    }
}
