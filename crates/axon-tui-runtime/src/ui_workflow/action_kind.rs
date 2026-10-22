//! UI action 可审计分类。

use crate::ui_workflow::action::UiAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum UiActionKind {
    PasteText,
    PasteFromSystemClipboard,
    PasteImageFromSystemClipboard,
    OpenExternalEditor,
    ToggleHistorySearch,
    RestoreInterruptInput,
    ClearDraft,
    OpenSelectedToolOutputDock,
    ToggleShortcutHelpDock,
    CloseShortcutHelpDock,
    CloseToolOutputDock,
    ScrollActiveSurfaceUp,
    ScrollActiveSurfaceDown,
    BottomViewKey,
    ComposerPopupPrevious,
    ComposerPopupNext,
    ComposerPopupAccept,
    ComposerPopupClose,
    CommandSuggestionPrevious,
    CommandSuggestionNext,
    CommandSuggestionAccept,
    CommandSuggestionAcceptForEnter,
    CompleteCommandTrigger,
    HistoryPrevious,
    HistoryNext,
    MoveCursorWordLeft,
    MoveCursorWordRight,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorHome,
    MoveCursorEnd,
    MoveCursorUp,
    MoveCursorDown,
    KillWordBeforeCursor,
    KillWordAfterCursor,
    Backspace,
    DeleteChar,
    KillToLineEnd,
    KillToLineStart,
    InsertSpace,
    InsertPlainChar,
    InsertNewline,
    SubmitCurrentDraft,
    SubmitSteer,
    InterruptRuntime,
    HistorySearchOlder,
    HistorySearchAccept,
    HistorySearchCancel,
    HistorySearchBackspace,
    HistorySearchPrevious,
    HistorySearchNext,
    HistorySearchPush,
    PageTranscriptViewUp,
    PageTranscriptViewDown,
    JumpTranscriptViewTop,
    JumpTranscriptViewBottom,
    VisibleTextSelectionBegin,
    VisibleTextSelectionDrag,
    VisibleTextSelectionRelease,
    ScrollToolOutputDockUp,
    ScrollToolOutputDockDown,
    JumpToolOutputDockTop,
    JumpToolOutputDockBottom,
    JumpToolOutputDockError,
}

impl UiActionKind {
    pub(crate) const fn as_str(self) -> &'static str {
        if let Some(name) = general_kind_name(self) {
            return name;
        }
        if let Some(name) = editor_kind_name(self) {
            return name;
        }
        if let Some(name) = runtime_kind_name(self) {
            return name;
        }
        transcript_or_surface_kind_name(self)
    }
}

const fn general_kind_name(kind: UiActionKind) -> Option<&'static str> {
    Some(match kind {
        UiActionKind::PasteText => "PasteText",
        UiActionKind::PasteFromSystemClipboard => "PasteFromSystemClipboard",
        UiActionKind::PasteImageFromSystemClipboard => "PasteImageFromSystemClipboard",
        UiActionKind::OpenExternalEditor => "OpenExternalEditor",
        UiActionKind::ToggleHistorySearch => "ToggleHistorySearch",
        UiActionKind::RestoreInterruptInput => "RestoreInterruptInput",
        UiActionKind::ClearDraft => "ClearDraft",
        UiActionKind::OpenSelectedToolOutputDock => "OpenSelectedToolOutputDock",
        UiActionKind::ToggleShortcutHelpDock => "ToggleShortcutHelpDock",
        UiActionKind::CloseShortcutHelpDock => "CloseShortcutHelpDock",
        UiActionKind::CloseToolOutputDock => "CloseToolOutputDock",
        UiActionKind::ScrollActiveSurfaceUp => "ScrollActiveSurfaceUp",
        UiActionKind::ScrollActiveSurfaceDown => "ScrollActiveSurfaceDown",
        UiActionKind::BottomViewKey => "BottomViewKey",
        UiActionKind::ComposerPopupPrevious => "ComposerPopupPrevious",
        UiActionKind::ComposerPopupNext => "ComposerPopupNext",
        UiActionKind::ComposerPopupAccept => "ComposerPopupAccept",
        UiActionKind::ComposerPopupClose => "ComposerPopupClose",
        UiActionKind::CommandSuggestionPrevious => "CommandSuggestionPrevious",
        UiActionKind::CommandSuggestionNext => "CommandSuggestionNext",
        UiActionKind::CommandSuggestionAccept => "CommandSuggestionAccept",
        UiActionKind::CommandSuggestionAcceptForEnter => "CommandSuggestionAcceptForEnter",
        UiActionKind::CompleteCommandTrigger => "CompleteCommandTrigger",
        _ => return None,
    })
}

const fn editor_kind_name(kind: UiActionKind) -> Option<&'static str> {
    Some(match kind {
        UiActionKind::HistoryPrevious => "HistoryPrevious",
        UiActionKind::HistoryNext => "HistoryNext",
        UiActionKind::MoveCursorWordLeft => "MoveCursorWordLeft",
        UiActionKind::MoveCursorWordRight => "MoveCursorWordRight",
        UiActionKind::MoveCursorLeft => "MoveCursorLeft",
        UiActionKind::MoveCursorRight => "MoveCursorRight",
        UiActionKind::MoveCursorHome => "MoveCursorHome",
        UiActionKind::MoveCursorEnd => "MoveCursorEnd",
        UiActionKind::MoveCursorUp => "MoveCursorUp",
        UiActionKind::MoveCursorDown => "MoveCursorDown",
        UiActionKind::KillWordBeforeCursor => "KillWordBeforeCursor",
        UiActionKind::KillWordAfterCursor => "KillWordAfterCursor",
        UiActionKind::Backspace => "Backspace",
        UiActionKind::DeleteChar => "DeleteChar",
        UiActionKind::KillToLineEnd => "KillToLineEnd",
        UiActionKind::KillToLineStart => "KillToLineStart",
        UiActionKind::InsertSpace => "InsertSpace",
        UiActionKind::InsertPlainChar => "InsertPlainChar",
        UiActionKind::InsertNewline => "InsertNewline",
        _ => return None,
    })
}

const fn runtime_kind_name(kind: UiActionKind) -> Option<&'static str> {
    Some(match kind {
        UiActionKind::SubmitCurrentDraft => "SubmitCurrentDraft",
        UiActionKind::SubmitSteer => "SubmitSteer",
        UiActionKind::InterruptRuntime => "InterruptRuntime",
        UiActionKind::HistorySearchOlder => "HistorySearchOlder",
        UiActionKind::HistorySearchAccept => "HistorySearchAccept",
        UiActionKind::HistorySearchCancel => "HistorySearchCancel",
        UiActionKind::HistorySearchBackspace => "HistorySearchBackspace",
        UiActionKind::HistorySearchPrevious => "HistorySearchPrevious",
        UiActionKind::HistorySearchNext => "HistorySearchNext",
        UiActionKind::HistorySearchPush => "HistorySearchPush",
        _ => return None,
    })
}

const fn transcript_or_surface_kind_name(kind: UiActionKind) -> &'static str {
    match kind {
        UiActionKind::PageTranscriptViewUp => "PageTranscriptViewUp",
        UiActionKind::PageTranscriptViewDown => "PageTranscriptViewDown",
        UiActionKind::JumpTranscriptViewTop => "JumpTranscriptViewTop",
        UiActionKind::JumpTranscriptViewBottom => "JumpTranscriptViewBottom",
        UiActionKind::VisibleTextSelectionBegin => "VisibleTextSelectionBegin",
        UiActionKind::VisibleTextSelectionDrag => "VisibleTextSelectionDrag",
        UiActionKind::VisibleTextSelectionRelease => "VisibleTextSelectionRelease",
        UiActionKind::ScrollToolOutputDockUp => "ScrollToolOutputDockUp",
        UiActionKind::ScrollToolOutputDockDown => "ScrollToolOutputDockDown",
        UiActionKind::JumpToolOutputDockTop => "JumpToolOutputDockTop",
        UiActionKind::JumpToolOutputDockBottom => "JumpToolOutputDockBottom",
        UiActionKind::JumpToolOutputDockError => "JumpToolOutputDockError",
        _ => unreachable!(),
    }
}

pub(crate) fn action_kind(action: &UiAction) -> Option<UiActionKind> {
    if matches!(
        action,
        UiAction::Noop | UiAction::Exit | UiAction::CloseTopDock
    ) {
        return None;
    }
    general_action_kind(action)
        .or_else(|| composer_action_kind(action))
        .or_else(|| search_action_kind(action))
        .or_else(|| transcript_action_kind(action))
        .or_else(|| mouse_action_kind(action))
        .or_else(|| tool_output_action_kind(action))
}

fn general_action_kind(action: &UiAction) -> Option<UiActionKind> {
    Some(match action {
        UiAction::PasteText(_) => UiActionKind::PasteText,
        UiAction::PasteFromSystemClipboard => UiActionKind::PasteFromSystemClipboard,
        UiAction::PasteImageFromSystemClipboard => UiActionKind::PasteImageFromSystemClipboard,
        UiAction::OpenExternalEditor => UiActionKind::OpenExternalEditor,
        UiAction::ToggleHistorySearch => UiActionKind::ToggleHistorySearch,
        UiAction::RestoreInterruptInput => UiActionKind::RestoreInterruptInput,
        UiAction::ClearDraft => UiActionKind::ClearDraft,
        UiAction::OpenSelectedToolOutputDock => UiActionKind::OpenSelectedToolOutputDock,
        UiAction::ToggleShortcutHelpDock => UiActionKind::ToggleShortcutHelpDock,
        UiAction::CloseShortcutHelpDock => UiActionKind::CloseShortcutHelpDock,
        UiAction::CloseToolOutputDock => UiActionKind::CloseToolOutputDock,
        UiAction::ScrollActiveSurfaceUp => UiActionKind::ScrollActiveSurfaceUp,
        UiAction::ScrollActiveSurfaceDown => UiActionKind::ScrollActiveSurfaceDown,
        UiAction::BottomViewKey(_) => UiActionKind::BottomViewKey,
        UiAction::InterruptRuntime => UiActionKind::InterruptRuntime,
        _ => return None,
    })
}

fn composer_action_kind(action: &UiAction) -> Option<UiActionKind> {
    Some(match action {
        UiAction::ComposerPopupPrevious => UiActionKind::ComposerPopupPrevious,
        UiAction::ComposerPopupNext => UiActionKind::ComposerPopupNext,
        UiAction::ComposerPopupAccept => UiActionKind::ComposerPopupAccept,
        UiAction::ComposerPopupClose => UiActionKind::ComposerPopupClose,
        UiAction::CommandSuggestionPrevious => UiActionKind::CommandSuggestionPrevious,
        UiAction::CommandSuggestionNext => UiActionKind::CommandSuggestionNext,
        UiAction::CommandSuggestionAccept => UiActionKind::CommandSuggestionAccept,
        UiAction::CommandSuggestionAcceptForEnter => UiActionKind::CommandSuggestionAcceptForEnter,
        UiAction::CompleteCommandTrigger => UiActionKind::CompleteCommandTrigger,
        UiAction::HistoryPrevious => UiActionKind::HistoryPrevious,
        UiAction::HistoryNext => UiActionKind::HistoryNext,
        UiAction::MoveCursorWordLeft => UiActionKind::MoveCursorWordLeft,
        UiAction::MoveCursorWordRight => UiActionKind::MoveCursorWordRight,
        UiAction::MoveCursorLeft => UiActionKind::MoveCursorLeft,
        UiAction::MoveCursorRight => UiActionKind::MoveCursorRight,
        UiAction::MoveCursorHome => UiActionKind::MoveCursorHome,
        UiAction::MoveCursorEnd => UiActionKind::MoveCursorEnd,
        UiAction::MoveCursorUp => UiActionKind::MoveCursorUp,
        UiAction::MoveCursorDown => UiActionKind::MoveCursorDown,
        UiAction::KillWordBeforeCursor => UiActionKind::KillWordBeforeCursor,
        UiAction::KillWordAfterCursor => UiActionKind::KillWordAfterCursor,
        UiAction::Backspace => UiActionKind::Backspace,
        UiAction::DeleteChar => UiActionKind::DeleteChar,
        UiAction::KillToLineEnd => UiActionKind::KillToLineEnd,
        UiAction::KillToLineStart => UiActionKind::KillToLineStart,
        UiAction::InsertSpace => UiActionKind::InsertSpace,
        UiAction::InsertPlainChar { .. } => UiActionKind::InsertPlainChar,
        UiAction::InsertNewline => UiActionKind::InsertNewline,
        UiAction::SubmitCurrentDraft { .. } => UiActionKind::SubmitCurrentDraft,
        UiAction::SubmitSteer { .. } => UiActionKind::SubmitSteer,
        _ => return None,
    })
}

fn search_action_kind(action: &UiAction) -> Option<UiActionKind> {
    Some(match action {
        UiAction::HistorySearchOlder => UiActionKind::HistorySearchOlder,
        UiAction::HistorySearchAccept => UiActionKind::HistorySearchAccept,
        UiAction::HistorySearchCancel => UiActionKind::HistorySearchCancel,
        UiAction::HistorySearchBackspace => UiActionKind::HistorySearchBackspace,
        UiAction::HistorySearchPrevious => UiActionKind::HistorySearchPrevious,
        UiAction::HistorySearchNext => UiActionKind::HistorySearchNext,
        UiAction::HistorySearchPush(_) => UiActionKind::HistorySearchPush,
        _ => return None,
    })
}

fn transcript_action_kind(action: &UiAction) -> Option<UiActionKind> {
    Some(match action {
        UiAction::PageTranscriptViewUp => UiActionKind::PageTranscriptViewUp,
        UiAction::PageTranscriptViewDown => UiActionKind::PageTranscriptViewDown,
        UiAction::JumpTranscriptViewTop => UiActionKind::JumpTranscriptViewTop,
        UiAction::JumpTranscriptViewBottom => UiActionKind::JumpTranscriptViewBottom,
        _ => return None,
    })
}

fn mouse_action_kind(action: &UiAction) -> Option<UiActionKind> {
    Some(match action {
        UiAction::VisibleTextSelectionBegin { .. } => UiActionKind::VisibleTextSelectionBegin,
        UiAction::VisibleTextSelectionDrag { .. } => UiActionKind::VisibleTextSelectionDrag,
        UiAction::VisibleTextSelectionRelease { .. } => UiActionKind::VisibleTextSelectionRelease,
        _ => return None,
    })
}

fn tool_output_action_kind(action: &UiAction) -> Option<UiActionKind> {
    Some(match action {
        UiAction::ScrollToolOutputDockUp => UiActionKind::ScrollToolOutputDockUp,
        UiAction::ScrollToolOutputDockDown => UiActionKind::ScrollToolOutputDockDown,
        UiAction::JumpToolOutputDockTop => UiActionKind::JumpToolOutputDockTop,
        UiAction::JumpToolOutputDockBottom => UiActionKind::JumpToolOutputDockBottom,
        UiAction::JumpToolOutputDockError => UiActionKind::JumpToolOutputDockError,
        _ => return None,
    })
}
