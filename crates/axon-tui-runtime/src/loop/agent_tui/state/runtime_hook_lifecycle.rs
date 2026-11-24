//! Runtime Hook transcript cell 生命周期编排。

use std::time::Instant;

use crate::r#loop::agent_tui::{
    run_output::{AgentTuiHookStatus, AgentTuiHookStatusView},
    state::{AgentTuiState, HOOK_QUIET_SUCCESS_LINGER, HOOK_REVEAL_DELAY},
};
use crate::surface::{
    cell::{RenderCellData, RenderCellRecord},
    cell_event_data::{AgentRenderHistoryCell, AgentRenderHookCell, AgentRenderLifecycleStatus},
};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_hook_status(&mut self, view: AgentTuiHookStatusView) {
        let hook_key = view.run_id.clone();
        if hook_status_is_quiet_success(&view) {
            self.complete_quiet_hook_success(&hook_key);
            return;
        }
        let line = hook_status_line(&view);
        if hook_status_line_is_running(&line) {
            self.stage_or_update_hook_cell(hook_key, line);
            return;
        }
        self.cells
            .active_runtime_cells
            .take_pending_hook_cell(&hook_key);
        self.cells.active_runtime_cells.clear_hook_run(&hook_key);
        self.remove_live_hook_item(&hook_key);
        if view.should_persist_history_line() {
            self.cells
                .transcript
                .push(persistent_hook_history_line(&view));
        }
    }

    pub(super) fn remove_active_hook_cell(&mut self, hook_key: &str) {
        self.cells.active_runtime_cells.clear_hook_run(hook_key);
        self.remove_live_hook_item(hook_key);
    }

    fn stage_or_update_hook_cell(&mut self, run_id: String, line: RenderCellRecord) {
        if self
            .cells
            .active_runtime_cells
            .hook_visible_since(&run_id)
            .is_some()
        {
            self.merge_or_replace_live_hook_line(line);
            return;
        }
        self.cells.active_runtime_cells.stage_hook_cell(
            run_id,
            line,
            Instant::now() + HOOK_REVEAL_DELAY,
        );
    }

    fn complete_quiet_hook_success(&mut self, run_id: &str) {
        let Some(visible_since) = self.cells.active_runtime_cells.hook_visible_since(run_id) else {
            self.cells
                .active_runtime_cells
                .take_pending_hook_cell(run_id);
            return;
        };
        let now = Instant::now();
        let removal_at = visible_since + HOOK_QUIET_SUCCESS_LINGER;
        if now >= removal_at {
            self.remove_active_hook_cell(run_id);
            return;
        }
        self.cells
            .active_runtime_cells
            .stage_quiet_hook_linger(run_id.to_owned(), removal_at);
    }
}

fn persistent_hook_history_line(view: &AgentTuiHookStatusView) -> RenderCellRecord {
    let line = view.persistent_history_line();
    RenderCellRecord::history_with_data(AgentRenderHistoryCell::from_tui_line(&line))
}

fn hook_status_line(view: &AgentTuiHookStatusView) -> RenderCellRecord {
    let cell = AgentRenderHookCell::from_tui_status(view);
    RenderCellRecord::hook_with_data(cell)
}

fn hook_status_is_quiet_success(view: &AgentTuiHookStatusView) -> bool {
    view.status == AgentTuiHookStatus::Completed
        && view.entries.is_empty()
        && view
            .status_message
            .as_deref()
            .is_none_or(|detail| detail.trim().is_empty())
}

fn hook_status_line_is_running(line: &RenderCellRecord) -> bool {
    let Some(RenderCellData::Hook(cell)) = &line.data else {
        return false;
    };
    cell.items
        .first()
        .is_none_or(|item| !hook_lifecycle_status_is_terminal(item.status))
}

fn hook_lifecycle_status_is_terminal(status: AgentRenderLifecycleStatus) -> bool {
    matches!(
        status,
        AgentRenderLifecycleStatus::Completed
            | AgentRenderLifecycleStatus::Failed
            | AgentRenderLifecycleStatus::Blocked
            | AgentRenderLifecycleStatus::Stopped
    )
}
