//! TUI 交互映射目录。

use crate::ui_workflow::{
    action::UiAction,
    action_kind::{UiActionKind, action_kind},
};

pub(crate) const MIN_TARGET_ACTION_COVERAGE: usize = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum UiShortcutScope {
    Paste,
    Mouse,
    ShortcutHelpDock,
    ToolOutputDock,
    HistorySearch,
    BottomView,
    ComposerPopup,
    CommandSuggestion,
    Global,
    EditorNavigation,
    Submit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UiShortcutBinding {
    pub(crate) scope: UiShortcutScope,
    pub(crate) gesture: &'static str,
    pub(crate) action: UiActionKind,
}

pub(crate) const UI_SHORTCUT_BINDINGS: &[UiShortcutBinding] = &[
    binding(
        UiShortcutScope::Paste,
        "terminal paste",
        UiActionKind::PasteText,
    ),
    binding(
        UiShortcutScope::Mouse,
        "wheel up in main transcript",
        UiActionKind::ScrollActiveSurfaceUp,
    ),
    binding(
        UiShortcutScope::Mouse,
        "wheel down in main transcript",
        UiActionKind::ScrollActiveSurfaceDown,
    ),
    binding(
        UiShortcutScope::Mouse,
        "start visible text selection",
        UiActionKind::VisibleTextSelectionBegin,
    ),
    binding(
        UiShortcutScope::Mouse,
        "drag visible text selection",
        UiActionKind::VisibleTextSelectionDrag,
    ),
    binding(
        UiShortcutScope::Mouse,
        "release visible text selection",
        UiActionKind::VisibleTextSelectionRelease,
    ),
    binding(
        UiShortcutScope::ShortcutHelpDock,
        "Esc/?",
        UiActionKind::CloseShortcutHelpDock,
    ),
    binding(
        UiShortcutScope::ToolOutputDock,
        "Esc",
        UiActionKind::CloseToolOutputDock,
    ),
    binding(
        UiShortcutScope::ToolOutputDock,
        "e",
        UiActionKind::JumpToolOutputDockError,
    ),
    binding(
        UiShortcutScope::ToolOutputDock,
        "PageUp",
        UiActionKind::ScrollToolOutputDockUp,
    ),
    binding(
        UiShortcutScope::ToolOutputDock,
        "PageDown",
        UiActionKind::ScrollToolOutputDockDown,
    ),
    binding(
        UiShortcutScope::ToolOutputDock,
        "Ctrl+Home",
        UiActionKind::JumpToolOutputDockTop,
    ),
    binding(
        UiShortcutScope::ToolOutputDock,
        "Ctrl+End",
        UiActionKind::JumpToolOutputDockBottom,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "Ctrl+R",
        UiActionKind::HistorySearchOlder,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "Enter",
        UiActionKind::HistorySearchAccept,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "Esc",
        UiActionKind::HistorySearchCancel,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "Backspace",
        UiActionKind::HistorySearchBackspace,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "Up",
        UiActionKind::HistorySearchPrevious,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "Down",
        UiActionKind::HistorySearchNext,
    ),
    binding(
        UiShortcutScope::HistorySearch,
        "text",
        UiActionKind::HistorySearchPush,
    ),
    binding(
        UiShortcutScope::BottomView,
        "any key",
        UiActionKind::BottomViewKey,
    ),
    binding(
        UiShortcutScope::ComposerPopup,
        "Up",
        UiActionKind::ComposerPopupPrevious,
    ),
    binding(
        UiShortcutScope::ComposerPopup,
        "Down",
        UiActionKind::ComposerPopupNext,
    ),
    binding(
        UiShortcutScope::ComposerPopup,
        "Tab/Enter",
        UiActionKind::ComposerPopupAccept,
    ),
    binding(
        UiShortcutScope::ComposerPopup,
        "Esc",
        UiActionKind::ComposerPopupClose,
    ),
    binding(
        UiShortcutScope::CommandSuggestion,
        "Up",
        UiActionKind::CommandSuggestionPrevious,
    ),
    binding(
        UiShortcutScope::CommandSuggestion,
        "Down",
        UiActionKind::CommandSuggestionNext,
    ),
    binding(
        UiShortcutScope::CommandSuggestion,
        "Tab",
        UiActionKind::CommandSuggestionAccept,
    ),
    binding(
        UiShortcutScope::CommandSuggestion,
        "Enter",
        UiActionKind::CommandSuggestionAcceptForEnter,
    ),
    binding(
        UiShortcutScope::CommandSuggestion,
        "Esc",
        UiActionKind::ClearDraft,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+L/Esc",
        UiActionKind::ClearDraft,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+R",
        UiActionKind::ToggleHistorySearch,
    ),
    binding(
        UiShortcutScope::Global,
        "Alt+V",
        UiActionKind::PasteImageFromSystemClipboard,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+V",
        UiActionKind::PasteFromSystemClipboard,
    ),
    binding(
        UiShortcutScope::Global,
        "Command+V",
        UiActionKind::PasteImageFromSystemClipboard,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+X",
        UiActionKind::OpenExternalEditor,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+O",
        UiActionKind::OpenSelectedToolOutputDock,
    ),
    binding(
        UiShortcutScope::Global,
        "PageUp",
        UiActionKind::PageTranscriptViewUp,
    ),
    binding(
        UiShortcutScope::Global,
        "PageDown",
        UiActionKind::PageTranscriptViewDown,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+Home",
        UiActionKind::JumpTranscriptViewTop,
    ),
    binding(
        UiShortcutScope::Global,
        "Ctrl+End",
        UiActionKind::JumpTranscriptViewBottom,
    ),
    binding(
        UiShortcutScope::Global,
        "?",
        UiActionKind::ToggleShortcutHelpDock,
    ),
    binding(
        UiShortcutScope::Global,
        "Esc while queued and working",
        UiActionKind::SubmitSteer,
    ),
    binding(
        UiShortcutScope::Global,
        "Esc while working",
        UiActionKind::InterruptRuntime,
    ),
    binding(
        UiShortcutScope::Global,
        "Alt+E",
        UiActionKind::RestoreInterruptInput,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Tab",
        UiActionKind::CompleteCommandTrigger,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Up",
        UiActionKind::HistoryPrevious,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Up in multiline input",
        UiActionKind::MoveCursorUp,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Down",
        UiActionKind::HistoryNext,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Down in multiline input",
        UiActionKind::MoveCursorDown,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Alt+Left",
        UiActionKind::MoveCursorWordLeft,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Alt+Right",
        UiActionKind::MoveCursorWordRight,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Left/Ctrl+B",
        UiActionKind::MoveCursorLeft,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Right/Ctrl+F",
        UiActionKind::MoveCursorRight,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Home/Ctrl+A",
        UiActionKind::MoveCursorHome,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "End/Ctrl+E",
        UiActionKind::MoveCursorEnd,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Backspace",
        UiActionKind::Backspace,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Delete",
        UiActionKind::DeleteChar,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Ctrl+K",
        UiActionKind::KillToLineEnd,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Ctrl+U",
        UiActionKind::KillToLineStart,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Ctrl+W",
        UiActionKind::KillWordBeforeCursor,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Alt+D",
        UiActionKind::KillWordAfterCursor,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "Space",
        UiActionKind::InsertSpace,
    ),
    binding(
        UiShortcutScope::EditorNavigation,
        "text",
        UiActionKind::InsertPlainChar,
    ),
    binding(
        UiShortcutScope::Submit,
        "Shift+Enter/Alt+Enter",
        UiActionKind::InsertNewline,
    ),
    binding(
        UiShortcutScope::Submit,
        "Enter",
        UiActionKind::SubmitCurrentDraft,
    ),
    binding(
        UiShortcutScope::Submit,
        "Enter while working",
        UiActionKind::SubmitCurrentDraft,
    ),
];

const fn binding(
    scope: UiShortcutScope,
    gesture: &'static str,
    action: UiActionKind,
) -> UiShortcutBinding {
    UiShortcutBinding {
        scope,
        gesture,
        action,
    }
}

pub(crate) fn action_has_catalog_binding(action: &UiAction) -> bool {
    debug_assert!(UI_SHORTCUT_BINDINGS.len() >= MIN_TARGET_ACTION_COVERAGE);
    let Some(kind) = action_kind(action) else {
        return matches!(
            action,
            UiAction::Noop | UiAction::Exit | UiAction::CloseTopDock
        );
    };
    UI_SHORTCUT_BINDINGS
        .iter()
        .any(|binding| binding.action == kind)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{MIN_TARGET_ACTION_COVERAGE, UI_SHORTCUT_BINDINGS, UiShortcutScope};

    #[test]
    fn catalog_covers_target_interaction_surfaces() {
        let scopes = UI_SHORTCUT_BINDINGS
            .iter()
            .map(|binding| binding.scope)
            .collect::<HashSet<_>>();

        for scope in [
            UiShortcutScope::Paste,
            UiShortcutScope::Mouse,
            UiShortcutScope::ShortcutHelpDock,
            UiShortcutScope::ToolOutputDock,
            UiShortcutScope::HistorySearch,
            UiShortcutScope::BottomView,
            UiShortcutScope::ComposerPopup,
            UiShortcutScope::CommandSuggestion,
            UiShortcutScope::Global,
            UiShortcutScope::EditorNavigation,
            UiShortcutScope::Submit,
        ] {
            assert!(scopes.contains(&scope), "{scope:?} 缺少映射目录");
        }

        let action_count = UI_SHORTCUT_BINDINGS
            .iter()
            .map(|binding| binding.action)
            .collect::<HashSet<_>>()
            .len();
        assert!(action_count >= MIN_TARGET_ACTION_COVERAGE);
    }

    #[test]
    fn ctrl_c_is_not_bound_as_tui_action() {
        assert!(
            UI_SHORTCUT_BINDINGS
                .iter()
                .all(|binding| binding.gesture != "Ctrl+C")
        );
    }
}
