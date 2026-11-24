//! Transcript cell history 预算维护。

use super::AgentTuiState;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn maintain_cell_history_budget(&mut self) -> bool {
        if !self.cells.transcript.exceeds_history_budget() {
            return false;
        }
        let target_len = self.cells.transcript.history_gc_target_len();
        let evicted = self
            .cells
            .transcript
            .evict_oldest_history_cells_to(target_len);
        let evicted_cell_ids = evicted
            .iter()
            .map(|(_, cell_id)| *cell_id)
            .collect::<Vec<_>>();
        self.viewport
            .transcript_render_cache
            .remove_cells(&evicted_cell_ids);
        self.viewport
            .transcript_height_cache
            .remove_cells(&evicted_cell_ids);
        for (index, _) in evicted.iter().rev() {
            self.rebase_transcript_indices_after_remove(*index);
        }
        self.prune_tool_output_buffers();
        !evicted.is_empty()
    }

    fn rebase_transcript_indices_after_remove(&mut self, removed_index: usize) {
        self.cells
            .active_runtime_cells
            .remove_transcript_index(removed_index);
        self.cells
            .transcript_surfaces
            .remove_transcript_index(removed_index);
        self.viewport
            .rebase_runtime_selection_after_remove(removed_index);
    }
}

#[cfg(test)]
mod tests {
    use super::super::runtime_flow_test_support::test_state;
    use crate::surface::{cell::RenderCellRecord, cell_store::CELL_HISTORY_MAX_CELLS};

    #[test]
    fn history_gc_reclaims_all_render_cache_entries_for_evicted_cells() {
        let mut state = test_state();
        state.cells.transcript.clear();
        state
            .cells
            .transcript
            .extend((0..=CELL_HISTORY_MAX_CELLS).map(|index| {
                RenderCellRecord::agent_message(
                    format!("assistant:test:history-gc:{index}"),
                    format!("历史 {index}"),
                )
            }));
        let evicted = state
            .cells
            .transcript
            .iter()
            .next()
            .cloned()
            .expect("最旧历史 cell 应存在");

        let _ = state
            .viewport
            .transcript_render_cache
            .render_cell_hyperlink_lines(&evicted, false, 80);
        let _ = state.viewport.transcript_height_cache.desired_height(
            &evicted,
            false,
            80,
            &state.viewport.transcript_render_cache,
        );
        assert_eq!(state.viewport.transcript_render_cache.entry_count(), 1);
        assert_eq!(state.viewport.transcript_height_cache.entry_count(), 1);

        assert!(state.maintain_cell_history_budget());

        assert_eq!(state.viewport.transcript_render_cache.entry_count(), 0);
        assert_eq!(state.viewport.transcript_height_cache.entry_count(), 0);
    }
}
