//! Runtime generation/status transcript cell 生命周期编排。

use crate::r#loop::agent_tui::{
    run_output::{
        AgentTuiLifecycleLine, AgentTuiLifecyclePhase, AgentTuiLifecycleStatusView,
        AgentTuiStatusItem, AgentTuiStatusLine, DaemonSubscriptionPhase,
    },
    state::{
        AgentRuntimeStatus, AgentTuiState,
        runtime_projection_policy::{RuntimeProjectionPolicy, RuntimeProjectionTarget},
    },
};
use crate::surface::cell::{
    RenderCellKind, RenderCellMode, RenderCellRecord, RenderCommandCell,
    RenderGenerationStatusCell, RenderStatusCell,
};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_subscription_phase(
        &mut self,
        phase: DaemonSubscriptionPhase,
    ) {
        if self.bridge.runtime_status == AgentRuntimeStatus::Working {
            self.bridge.subscription_phase = Some(phase);
        }
    }

    pub(in crate::r#loop::agent_tui) fn apply_lifecycle_status(
        &mut self,
        view: AgentTuiLifecycleStatusView,
    ) {
        if self.push_history_lifecycle_status(&view) {
            return;
        }
        let active_lines = self.active_transcript_lines();
        if RuntimeProjectionPolicy::lifecycle_status_target(&view, &active_lines)
            == RuntimeProjectionTarget::Drop
        {
            return;
        }
        let Some(line) = lifecycle_status_line(view) else {
            return;
        };
        let kind = lifecycle_line_kind(&line);
        self.replace_or_push_lifecycle_cell(kind, line);
    }

    pub(in crate::r#loop::agent_tui) fn replace_or_append_active_status_cell(
        &mut self,
        status: AgentTuiStatusLine,
    ) {
        let line = status_line(status);
        let kind = lifecycle_line_kind(&line);
        self.replace_or_push_lifecycle_cell(kind, line);
    }

    pub(in crate::r#loop::agent_tui) fn clear_active_lifecycle_cells(&mut self) {
        let mut indices = self
            .cells
            .active_runtime_cells
            .take_command_cells()
            .into_iter()
            .chain(
                self.cells
                    .active_runtime_cells
                    .take_history_lifecycle_cells(),
            )
            .map(|part| part.index)
            .filter(|index| *index < self.cells.transcript.len())
            .collect::<Vec<_>>();
        indices.sort_unstable_by(|left, right| right.cmp(left));
        indices.dedup();

        for index in indices {
            self.cells.transcript.remove(index);
            self.cells
                .active_runtime_cells
                .remove_transcript_index(index);
            self.cells
                .transcript_surfaces
                .remove_transcript_index(index);
        }
    }

    pub(in crate::r#loop::agent_tui) fn replace_or_push_lifecycle_cell(
        &mut self,
        kind: RenderCellKind,
        line: RenderCellRecord,
    ) {
        match RuntimeProjectionPolicy::lifecycle_cell_target(&line) {
            RuntimeProjectionTarget::LiveTail | RuntimeProjectionTarget::DockStatus => {
                self.merge_or_replace_live_lifecycle_line(line);
                return;
            }
            RuntimeProjectionTarget::Drop => return,
            RuntimeProjectionTarget::Transcript => {}
        }
        let Some(part_id) = line.semantic_part_id() else {
            return;
        };
        let replace_index = self.take_lifecycle_replacement(kind, part_id.as_str());
        if let Some(index) = replace_index
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            existing.replace_with(line);
            let Some(part_id) = existing.semantic_part_id() else {
                return;
            };
            self.set_lifecycle_cell(kind, part_id, index);
            return;
        }
        self.cells.transcript.push(line);
        let index = self.cells.transcript.len().saturating_sub(1);
        let Some(part_id) = self
            .cells
            .transcript
            .get(index)
            .and_then(RenderCellRecord::semantic_part_id)
        else {
            return;
        };
        self.set_lifecycle_cell(kind, part_id, index);
    }

    fn take_lifecycle_replacement(&mut self, kind: RenderCellKind, part_id: &str) -> Option<usize> {
        match kind {
            RenderCellKind::COMMAND => self
                .cells
                .active_runtime_cells
                .take_command_cell(part_id)
                .map(|part| part.index),
            _ => None,
        }
    }

    fn set_lifecycle_cell(&mut self, kind: RenderCellKind, part_id: String, index: usize) {
        if kind == RenderCellKind::COMMAND {
            self.cells
                .active_runtime_cells
                .set_command_cell(part_id, index);
        }
    }
}

fn lifecycle_status_line(view: AgentTuiLifecycleStatusView) -> Option<RenderCellRecord> {
    let phase = view.phase;
    match phase {
        AgentTuiLifecyclePhase::Streaming | AgentTuiLifecyclePhase::Generation => {
            let line =
                AgentTuiLifecycleLine::with_id(view.id.clone(), view.title.clone(), vec![view]);
            Some(RenderCellRecord::generation_status_with_data(
                RenderGenerationStatusCell::from_lifecycle_line(&line),
            ))
        }
        AgentTuiLifecyclePhase::Command => {
            let id = format!("command:{}", view.id);
            let title = view.title.clone();
            Some(RenderCellRecord::command_with_data(
                RenderCommandCell::from_tui_status(&AgentTuiStatusLine::with_id(
                    id,
                    title,
                    vec![AgentTuiStatusItem::from_lifecycle_view(view)],
                )),
            ))
        }
        AgentTuiLifecyclePhase::Notice => {
            let id = format!("status:{}", view.id);
            let title = view.title.clone();
            Some(RenderCellRecord::status_with_data(
                RenderStatusCell::from_tui_status(&AgentTuiStatusLine::with_id(
                    id,
                    title,
                    vec![AgentTuiStatusItem::from_lifecycle_view(view)],
                )),
            ))
        }
        AgentTuiLifecyclePhase::Unknown => None,
    }
}

fn status_line(status: AgentTuiStatusLine) -> RenderCellRecord {
    RenderCellRecord::status_with_data(RenderStatusCell::from_tui_status(&status))
}

fn lifecycle_line_kind(line: &RenderCellRecord) -> RenderCellKind {
    match line.mode {
        RenderCellMode::Cell(kind) => kind,
        RenderCellMode::Line | RenderCellMode::Startup => RenderCellKind::STATUS,
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentRuntimeStatus, AgentTuiState, DaemonSubscriptionPhase};
    use crate::r#loop::agent_tui::state_tests::launch_request;

    #[test]
    fn subscription_phase_is_stored_only_for_working_turn() {
        let mut state = AgentTuiState::new_for_test(&launch_request(None));

        state.apply_subscription_phase(DaemonSubscriptionPhase::Resyncing);
        assert_eq!(state.bridge.subscription_phase, None);

        state.bridge.runtime_status = AgentRuntimeStatus::Working;
        state.apply_subscription_phase(DaemonSubscriptionPhase::Reconnecting);
        assert_eq!(
            state.bridge.subscription_phase,
            Some(DaemonSubscriptionPhase::Reconnecting)
        );

        state.apply_subscription_phase(DaemonSubscriptionPhase::Resyncing);
        assert_eq!(
            state.bridge.subscription_phase,
            Some(DaemonSubscriptionPhase::Resyncing)
        );
    }
}
