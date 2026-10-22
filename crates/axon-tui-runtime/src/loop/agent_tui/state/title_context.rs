//! Agent TUI 终端标题上下文状态逻辑。

use std::path::Path;

use super::super::{
    state::AgentTuiState,
    state::ui_mode::UiActivity,
    terminal_title::{TerminalTitleActivity, TerminalTitleContext},
};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn terminal_title_context(&self) -> TerminalTitleContext<'_> {
        debug_assert!(!self.bridge.runtime_status.as_str().is_empty());
        let ui = self.ui_mode_snapshot();
        TerminalTitleContext {
            conversation_title: self.current_session_title(),
            workspace_name: self.current_workspace_name(),
            activity: terminal_title_activity(ui.activity()),
            lifecycle_revision: self.bridge.terminal_title_revision,
        }
    }

    pub(in crate::r#loop::agent_tui) fn advance_terminal_title_lifecycle(&mut self) {
        self.bridge.terminal_title_revision = self.bridge.terminal_title_revision.wrapping_add(1);
    }

    fn current_session_title(&self) -> Option<&str> {
        self.header
            .session_summary
            .title
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| self.current_catalog_session_title())
    }

    fn current_catalog_session_title(&self) -> Option<&str> {
        let session_id = self.header.overrides.session.as_deref().or(self
            .header
            .session_summary
            .session_id
            .as_deref())?;
        self.header
            .catalog
            .sessions
            .iter()
            .find(|session| session.session_id == session_id)
            .and_then(|session| session.title.as_deref())
            .filter(|value| !value.trim().is_empty())
    }

    fn current_workspace_name(&self) -> Option<&str> {
        Path::new(&self.header.cwd)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|value| !value.trim().is_empty())
    }
}

const fn terminal_title_activity(activity: UiActivity) -> TerminalTitleActivity {
    match activity {
        UiActivity::Idle => TerminalTitleActivity::Idle,
        UiActivity::Queued => TerminalTitleActivity::Queued,
        UiActivity::Working => TerminalTitleActivity::Working,
    }
}
