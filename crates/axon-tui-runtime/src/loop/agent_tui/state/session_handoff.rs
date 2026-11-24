//! Agent TUI 内部 runtime 切换时的瞬时 UI 状态清理。

use super::AgentTuiState;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn clear_transient_runtime_state(&mut self) {
        self.header.clear_handoff_state();
        self.composer.clear_handoff_state();
        self.viewport.clear_handoff_state();
        self.cells.active_runtime_cells.clear();
    }
}
