//! Composer 历史导航与历史搜索状态。

use crate::r#loop::agent_tui::{TUI_HINT_IDLE, state::HISTORY_SEARCH_HINT};

use super::AgentTuiState;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn history_previous(&mut self) {
        if self.composer.history.is_empty() {
            return;
        }
        if self.composer.history_index.is_none() {
            self.composer.history_navigation_draft = Some(self.composer.draft.clone());
        }
        let index = self.composer.history_index.map_or_else(
            || self.composer.history.len().saturating_sub(1),
            |value| value.saturating_sub(1),
        );
        self.apply_history_index(index);
    }

    pub(in crate::r#loop::agent_tui) fn history_next(&mut self) {
        let Some(index) = self.composer.history_index else {
            return;
        };
        if index + 1 >= self.composer.history.len() {
            self.restore_history_navigation_draft();
            return;
        }
        self.apply_history_index(index + 1);
    }

    pub(in crate::r#loop::agent_tui) fn toggle_history_search(&mut self) {
        self.composer.history_navigation_draft = Some(self.composer.draft.clone());
        let current = self
            .composer
            .history_search
            .begin(&self.composer.history, self.composer.draft.text());
        self.apply_history_search_current(current);
        self.composer.history_index = None;
        self.header.hint = HISTORY_SEARCH_HINT;
    }

    pub(in crate::r#loop::agent_tui) fn is_history_searching(&self) -> bool {
        self.composer.history_search.is_active()
    }

    pub(in crate::r#loop::agent_tui) fn push_history_search(&mut self, value: char) {
        let current = self
            .composer
            .history_search
            .push(&self.composer.history, value);
        self.apply_history_search_current(current);
    }

    pub(in crate::r#loop::agent_tui) fn continue_history_search_older(&mut self) {
        let current = self
            .composer
            .history_search
            .previous(&self.composer.history);
        self.apply_history_search_current(current);
    }

    pub(in crate::r#loop::agent_tui) fn backspace_history_search(&mut self) {
        let current = self
            .composer
            .history_search
            .backspace(&self.composer.history);
        self.apply_history_search_current(current);
    }

    pub(in crate::r#loop::agent_tui) fn history_search_previous(&mut self) {
        let current = self
            .composer
            .history_search
            .previous(&self.composer.history);
        self.apply_history_search_current(current);
    }

    pub(in crate::r#loop::agent_tui) fn history_search_next(&mut self) {
        let current = self.composer.history_search.next(&self.composer.history);
        self.apply_history_search_current(current);
    }

    pub(in crate::r#loop::agent_tui) fn accept_history_search(&mut self) {
        let draft = self.composer.history_search.accept(&self.composer.history);
        self.composer.history_navigation_draft = None;
        self.replace_draft(draft);
        self.header.hint = TUI_HINT_IDLE;
    }

    pub(in crate::r#loop::agent_tui) fn cancel_history_search(&mut self) {
        let fallback = self.composer.history_search.cancel();
        if let Some(draft) = self.composer.history_navigation_draft.take() {
            self.composer.draft = draft;
            self.composer.history_index = None;
            self.refresh_command_suggestions();
        } else {
            self.replace_draft(fallback);
        }
        self.header.hint = TUI_HINT_IDLE;
    }

    fn apply_history_search_current(&mut self, current: Option<String>) {
        self.composer.draft.replace_all(current.unwrap_or_default());
        self.refresh_command_suggestions();
    }

    fn apply_history_index(&mut self, index: usize) {
        self.composer.history_index = Some(index);
        self.composer
            .draft
            .replace_all(self.composer.history[index].clone());
        self.refresh_command_suggestions();
    }

    fn restore_history_navigation_draft(&mut self) {
        let draft = self
            .composer
            .history_navigation_draft
            .take()
            .unwrap_or_default();
        self.composer.history_index = None;
        self.composer.draft = draft;
        self.refresh_command_suggestions();
    }
}
