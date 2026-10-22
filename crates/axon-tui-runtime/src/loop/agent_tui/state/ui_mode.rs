//! Agent TUI 会话级交互模式派生。

use crate::app_shell::composer_dock::DockKind;

use super::AgentTuiState;

/// 当前 TUI 的主交互模式。队列是正交状态，不参与模式优先级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) enum UiMode {
    Dock(DockKind),
    HistorySearch,
    ComposerPopup,
    Working,
    Editing,
    Ready,
}

/// 输入区下方状态行、TerminalTitle 与输入提示共用的运行活动态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) enum UiActivity {
    Idle,
    Queued,
    Working,
}

/// 输入区下方状态行与 TerminalTitle 共用的状态投影。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct UiModeSnapshot {
    pub(in crate::r#loop::agent_tui) mode: UiMode,
    activity: UiActivity,
}

impl UiModeSnapshot {
    pub(in crate::r#loop::agent_tui) const fn activity(self) -> UiActivity {
        self.activity
    }
}

impl UiActivity {
    pub(in crate::r#loop::agent_tui) const fn from_state(working: bool, queued: bool) -> Self {
        if working {
            Self::Working
        } else if queued {
            Self::Queued
        } else {
            Self::Idle
        }
    }
}

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn ui_mode_snapshot(&self) -> UiModeSnapshot {
        let mode = self.ui_mode();
        UiModeSnapshot {
            mode,
            activity: UiActivity::from_state(self.is_working(), self.has_queued_submit()),
        }
    }

    pub(in crate::r#loop::agent_tui) fn ui_mode(&self) -> UiMode {
        if let Some(kind) = self.active_dock_kind() {
            return UiMode::Dock(kind);
        }
        if self.is_history_searching() {
            return UiMode::HistorySearch;
        }
        if self.composer_popup_open() || self.command_suggestions_open() {
            return UiMode::ComposerPopup;
        }
        if self.is_working() {
            return UiMode::Working;
        }
        if !self.composer.draft.text().is_empty() {
            return UiMode::Editing;
        }
        UiMode::Ready
    }
}
