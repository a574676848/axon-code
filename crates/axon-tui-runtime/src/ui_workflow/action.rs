//! TUI 本地动作枚举。

use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiKeyCode {
    Enter,
    Tab,
    Esc,
    Up,
    Down,
    Backspace,
    Char(char),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UiKeyModifiers {
    pub(crate) control: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UiKeyEvent {
    pub(crate) code: UiKeyCode,
    pub(crate) modifiers: UiKeyModifiers,
}

#[cfg(test)]
impl UiKeyEvent {
    pub(crate) const fn plain(code: UiKeyCode) -> Self {
        Self {
            code,
            modifiers: UiKeyModifiers::NONE,
        }
    }
}

impl UiKeyModifiers {
    #[cfg(test)]
    pub(crate) const NONE: Self = Self {
        control: false,
        alt: false,
        shift: false,
    };

    pub(crate) const fn is_plain(self) -> bool {
        !self.control && !self.alt && !self.shift
    }

    pub(crate) const fn is_plain_or_shift(self) -> bool {
        !self.control && !self.alt
    }
}

/// UI action 是本地交互的唯一入口，runtime event 不进入这里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UiAction {
    Noop,
    Exit,
    PasteText(String),
    PasteFromSystemClipboard,
    PasteImageFromSystemClipboard,
    OpenExternalEditor,
    ToggleHistorySearch,
    RestoreInterruptInput,
    ClearDraft,
    OpenSelectedToolOutputDock,
    ToggleShortcutHelpDock,
    CloseTopDock,
    CloseShortcutHelpDock,
    CloseToolOutputDock,
    ScrollActiveSurfaceUp,
    ScrollActiveSurfaceDown,
    BottomViewKey(UiKeyEvent),
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
    InsertPlainChar { value: char, now: Instant },
    InsertNewline,
    SubmitCurrentDraft { now: Instant },
    SubmitSteer { now: Instant },
    InterruptRuntime,
    HistorySearchOlder,
    HistorySearchAccept,
    HistorySearchCancel,
    HistorySearchBackspace,
    HistorySearchPrevious,
    HistorySearchNext,
    HistorySearchPush(char),
    PageTranscriptViewUp,
    PageTranscriptViewDown,
    JumpTranscriptViewTop,
    JumpTranscriptViewBottom,
    VisibleTextSelectionBegin { row: u16, column: u16 },
    VisibleTextSelectionDrag { row: u16, column: u16 },
    VisibleTextSelectionRelease { row: u16, column: u16, now: Instant },
    ScrollToolOutputDockUp,
    ScrollToolOutputDockDown,
    JumpToolOutputDockTop,
    JumpToolOutputDockBottom,
    JumpToolOutputDockError,
}
