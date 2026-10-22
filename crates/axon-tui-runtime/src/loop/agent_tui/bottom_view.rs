//! Agent TUI 底部交互视图聚合。

#[path = "bottom_view/question.rs"]
pub(crate) mod question;
#[path = "bottom_view/selection.rs"]
mod selection;
#[path = "bottom_view/skills_toggle.rs"]
mod skills_toggle;
#[path = "bottom_view/stack.rs"]
mod stack;

use crate::app_shell::composer_dock::DockKind;
use crate::r#loop::agent_tui::{output_dock::ToolOutputDock, shortcut_help::ShortcutEntry};
use crate::ui_workflow::action::UiKeyEvent;

pub(crate) use question::QuestionView;
pub(super) use selection::BottomViewAction;
pub(crate) use selection::SelectionView;
pub(super) use skills_toggle::SkillsToggleSummary;
pub(crate) use skills_toggle::SkillsToggleView;
pub(crate) use stack::BottomViewStack;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BottomView {
    Question(QuestionView),
    Selection(SelectionView),
    SkillsToggle(SkillsToggleView),
    ToolOutputViewer(ToolOutputDock),
    ShortcutHelp(ShortcutHelpDock),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BottomViewEvent {
    None,
    Close,
    Action(BottomViewAction),
    SkillsClosed(SkillsToggleSummary),
    QuestionAnswered {
        lane_id: String,
        call_id: String,
        answers: Vec<axon_domain::UserAnswerItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShortcutHelpDock {
    entries: &'static [ShortcutEntry],
}

impl ShortcutHelpDock {
    pub(crate) fn new(entries: &'static [ShortcutEntry]) -> Self {
        Self { entries }
    }

    pub(crate) fn entries(&self) -> &'static [ShortcutEntry] {
        self.entries
    }
}

impl BottomView {
    pub(super) fn handle_key(&mut self, key: UiKeyEvent) -> BottomViewEvent {
        match self {
            Self::Question(view) => view.handle_key(key),
            Self::Selection(view) => view.handle_key(key),
            Self::SkillsToggle(view) => view.handle_key(key),
            Self::ToolOutputViewer(_) | Self::ShortcutHelp(_) => BottomViewEvent::None,
        }
    }

    pub(crate) fn height(&self) -> u16 {
        match self {
            Self::Question(view) => view.height(),
            Self::Selection(view) => view.height(),
            Self::SkillsToggle(view) => view.height(),
            Self::ToolOutputViewer(_) => 8,
            Self::ShortcutHelp(_) => 8,
        }
    }

    pub(crate) fn kind(&self) -> DockKind {
        match self {
            Self::Question(view) => view.kind(),
            Self::Selection(view) => view.kind(),
            Self::SkillsToggle(_) => DockKind::Skills,
            Self::ToolOutputViewer(_) => DockKind::ToolOutputViewer,
            Self::ShortcutHelp(_) => DockKind::ShortcutHelp,
        }
    }

    pub(crate) fn tool_output(&self) -> Option<&ToolOutputDock> {
        match self {
            Self::ToolOutputViewer(view) => Some(view),
            _ => None,
        }
    }

    pub(crate) fn tool_output_mut(&mut self) -> Option<&mut ToolOutputDock> {
        match self {
            Self::ToolOutputViewer(view) => Some(view),
            _ => None,
        }
    }
}
