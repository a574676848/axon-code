//! Composer 输入区状态事实源。

use std::path::PathBuf;

use crate::r#loop::agent_tui::{
    bottom_view::BottomViewStack,
    commands::AgentTuiCommandSuggestion,
    composer_popup::{ComposerPopup, PathCompletionState},
    draft_state::DraftState,
    history_search::HistorySearch,
    state::InputQueueState,
};

/// Composer、历史导航、popup 和队列输入状态。
#[derive(Debug, Clone)]
pub(crate) struct ComposerState {
    pub(crate) draft: DraftState,
    pub(crate) submitted_prompt: Option<String>,
    pub(crate) command_suggestions: Vec<AgentTuiCommandSuggestion>,
    pub(crate) selected_command_suggestion: usize,
    pub(crate) composer_popup: Option<ComposerPopup>,
    pub(crate) path_completion: PathCompletionState,
    pub(crate) bottom_views: BottomViewStack,
    pub(crate) history: Vec<String>,
    pub(crate) history_path: PathBuf,
    pub(crate) history_search: HistorySearch,
    pub(crate) history_index: Option<usize>,
    pub(crate) history_navigation_draft: Option<DraftState>,
    pub(crate) input_queue: InputQueueState,
}

impl ComposerState {
    pub(crate) fn new(history: Vec<String>, history_path: PathBuf) -> Self {
        Self {
            draft: DraftState::default(),
            submitted_prompt: None,
            command_suggestions: Vec::new(),
            selected_command_suggestion: 0,
            composer_popup: None,
            path_completion: PathCompletionState::default(),
            bottom_views: BottomViewStack::default(),
            history,
            history_path,
            history_search: HistorySearch::default(),
            history_index: None,
            history_navigation_draft: None,
            input_queue: InputQueueState::default(),
        }
    }

    pub(crate) fn clear_handoff_state(&mut self) {
        self.draft = DraftState::default();
        self.submitted_prompt = None;
        self.command_suggestions.clear();
        self.selected_command_suggestion = 0;
        self.composer_popup = None;
        self.path_completion.clear();
        self.bottom_views.clear();
        self.history_search = HistorySearch::default();
        self.history_index = None;
        self.history_navigation_draft = None;
        self.input_queue = InputQueueState::default();
    }
}
