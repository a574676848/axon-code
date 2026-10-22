//! Agent TUI 键盘事件语义映射。

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::{
    app_shell::composer_dock::DockKind,
    r#loop::agent_tui::{shortcuts as terminal_shortcuts, state::AgentTuiState},
    ui_workflow::action::{UiAction, UiKeyCode, UiKeyEvent, UiKeyModifiers},
};

pub(super) fn map_key_to_action(state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return None;
    }
    if let Some(action) = map_active_dock_key_to_action(state, key) {
        return Some(action);
    }
    if state.is_history_searching() {
        return map_history_search_key_to_action(key);
    }
    if state.bottom_view_open() {
        return Some(UiAction::BottomViewKey(ui_key_event(key)));
    }
    if let Some(action) = map_composer_popup_key_to_action(state, key) {
        return Some(action);
    }
    if let Some(action) = map_command_suggestion_key_to_action(state, key) {
        return Some(action);
    }
    if let Some(action) = map_global_key_to_action(state, key) {
        return Some(action);
    }
    if let Some(action) = map_editor_navigation_key_to_action(state, key) {
        return Some(action);
    }
    map_submit_key_to_action(state, key)
}

fn ui_key_event(key: KeyEvent) -> UiKeyEvent {
    UiKeyEvent {
        code: match key.code {
            KeyCode::Enter => UiKeyCode::Enter,
            KeyCode::Tab => UiKeyCode::Tab,
            KeyCode::Esc => UiKeyCode::Esc,
            KeyCode::Up => UiKeyCode::Up,
            KeyCode::Down => UiKeyCode::Down,
            KeyCode::Backspace => UiKeyCode::Backspace,
            KeyCode::Char(value) => UiKeyCode::Char(value),
            _ => UiKeyCode::Other,
        },
        modifiers: UiKeyModifiers {
            control: key.modifiers.contains(KeyModifiers::CONTROL),
            alt: key.modifiers.contains(KeyModifiers::ALT),
            shift: key.modifiers.contains(KeyModifiers::SHIFT),
        },
    }
}

fn map_active_dock_key_to_action(state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    match state.active_dock_kind()? {
        DockKind::ShortcutHelp => Some(map_shortcut_help_dock_key_to_action(key)),
        DockKind::ToolOutputViewer => Some(map_tool_output_dock_key_to_action(key)),
        _ if terminal_shortcuts::is_exit_key(&key) => Some(UiAction::CloseTopDock),
        _ => None,
    }
}

fn map_composer_popup_key_to_action(state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    if !state.composer_popup_open() {
        return None;
    }
    match key.code {
        KeyCode::Up => Some(UiAction::ComposerPopupPrevious),
        KeyCode::Down => Some(UiAction::ComposerPopupNext),
        KeyCode::Tab | KeyCode::Enter => Some(UiAction::ComposerPopupAccept),
        KeyCode::Esc if terminal_shortcuts::is_exit_key(&key) => Some(UiAction::ComposerPopupClose),
        _ => None,
    }
}

fn map_command_suggestion_key_to_action(state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    if !state.command_suggestions_open() {
        return None;
    }
    match key.code {
        KeyCode::Up => Some(UiAction::CommandSuggestionPrevious),
        KeyCode::Down => Some(UiAction::CommandSuggestionNext),
        KeyCode::Tab => Some(UiAction::CommandSuggestionAccept),
        KeyCode::Enter => Some(UiAction::CommandSuggestionAcceptForEnter),
        KeyCode::Esc if terminal_shortcuts::is_exit_key(&key) => Some(UiAction::ClearDraft),
        _ => None,
    }
}

fn map_global_key_to_action(state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::ClearDraft)
        }
        KeyCode::Char('r')
            if key.modifiers.contains(KeyModifiers::CONTROL) && !state.is_working() =>
        {
            Some(UiAction::ToggleHistorySearch)
        }
        KeyCode::Char('v' | 'V')
            if key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            Some(UiAction::PasteImageFromSystemClipboard)
        }
        KeyCode::Char('v' | 'V')
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(UiAction::PasteFromSystemClipboard)
        }
        KeyCode::Char('v' | 'V')
            if key.modifiers.contains(KeyModifiers::SUPER)
                && !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
        {
            Some(UiAction::PasteImageFromSystemClipboard)
        }
        KeyCode::Char('x')
            if key.modifiers.contains(KeyModifiers::CONTROL) && !state.is_working() =>
        {
            Some(UiAction::OpenExternalEditor)
        }
        KeyCode::PageUp if key.modifiers == KeyModifiers::NONE => {
            Some(UiAction::PageTranscriptViewUp)
        }
        KeyCode::PageDown if key.modifiers == KeyModifiers::NONE => {
            Some(UiAction::PageTranscriptViewDown)
        }
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::JumpTranscriptViewTop)
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::JumpTranscriptViewBottom)
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::OpenSelectedToolOutputDock)
        }
        KeyCode::Char('?')
            if key.modifiers == KeyModifiers::NONE
                && state.composer.draft.text().trim().is_empty() =>
        {
            Some(UiAction::ToggleShortcutHelpDock)
        }
        KeyCode::Esc
            if terminal_shortcuts::is_runtime_exit_key(&key)
                && state.is_working()
                && !state.has_pending_steer()
                && state.has_queued_follow_up_message() =>
        {
            Some(UiAction::SubmitSteer {
                now: Instant::now(),
            })
        }
        KeyCode::Esc if terminal_shortcuts::is_runtime_exit_key(&key) && state.is_working() => {
            Some(UiAction::InterruptRuntime)
        }
        KeyCode::Esc if terminal_shortcuts::is_exit_key(&key) => Some(UiAction::ClearDraft),
        KeyCode::Char('e' | 'E') if key.modifiers.contains(KeyModifiers::ALT) => {
            Some(UiAction::RestoreInterruptInput)
        }
        _ => None,
    }
}

fn map_editor_navigation_key_to_action(state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Tab if !state.is_working() => Some(UiAction::CompleteCommandTrigger),
        KeyCode::Up
            if key.modifiers.is_empty()
                && unchanged_recalled_history_entry(state)
                && state.draft_cursor_is_on_first_visual_line() =>
        {
            Some(UiAction::HistoryPrevious)
        }
        KeyCode::Down
            if key.modifiers.is_empty()
                && unchanged_recalled_history_entry(state)
                && state.draft_cursor_is_on_last_visual_line() =>
        {
            Some(UiAction::HistoryNext)
        }
        KeyCode::Up if key.modifiers.is_empty() && state.draft_has_multiple_visual_lines() => {
            Some(UiAction::MoveCursorUp)
        }
        KeyCode::Down if key.modifiers.is_empty() && state.draft_has_multiple_visual_lines() => {
            Some(UiAction::MoveCursorDown)
        }
        KeyCode::Up if key.modifiers.is_empty() => Some(UiAction::HistoryPrevious),
        KeyCode::Down if key.modifiers.is_empty() => Some(UiAction::HistoryNext),
        KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
            Some(UiAction::MoveCursorWordLeft)
        }
        KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
            Some(UiAction::MoveCursorWordRight)
        }
        KeyCode::Left | KeyCode::Char('b') if cursor_control_key(&key, 'b') => {
            Some(UiAction::MoveCursorLeft)
        }
        KeyCode::Right | KeyCode::Char('f') if cursor_control_key(&key, 'f') => {
            Some(UiAction::MoveCursorRight)
        }
        KeyCode::Home | KeyCode::Char('a') if cursor_control_key(&key, 'a') => {
            Some(UiAction::MoveCursorHome)
        }
        KeyCode::End | KeyCode::Char('e') if cursor_control_key(&key, 'e') => {
            Some(UiAction::MoveCursorEnd)
        }
        KeyCode::Backspace
            if key.modifiers.contains(KeyModifiers::ALT)
                || key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            Some(UiAction::KillWordBeforeCursor)
        }
        KeyCode::Delete
            if key.modifiers.contains(KeyModifiers::ALT)
                || key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            Some(UiAction::KillWordAfterCursor)
        }
        KeyCode::Backspace => Some(UiAction::Backspace),
        KeyCode::Delete => Some(UiAction::DeleteChar),
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::KillToLineEnd)
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::KillToLineStart)
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::KillWordBeforeCursor)
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::ALT) => {
            Some(UiAction::KillWordAfterCursor)
        }
        KeyCode::Char(' ') if key.modifiers.is_empty() => Some(UiAction::InsertSpace),
        KeyCode::Char(value) if terminal_shortcuts::is_text_input_key(&key) => {
            Some(UiAction::InsertPlainChar {
                value,
                now: Instant::now(),
            })
        }
        _ => None,
    }
}

fn unchanged_recalled_history_entry(state: &AgentTuiState) -> bool {
    state
        .composer
        .history_index
        .and_then(|index| state.composer.history.get(index))
        .is_some_and(|entry| entry == state.composer.draft.text())
}

fn cursor_control_key(key: &KeyEvent, control_char: char) -> bool {
    match key.code {
        KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => key.modifiers.is_empty(),
        KeyCode::Char(value) => {
            value == control_char && key.modifiers.contains(KeyModifiers::CONTROL)
        }
        _ => false,
    }
}

fn map_submit_key_to_action(_state: &AgentTuiState, key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Enter
            if key.modifiers.contains(KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(UiAction::InsertNewline)
        }
        KeyCode::Enter => Some(UiAction::SubmitCurrentDraft {
            now: Instant::now(),
        }),
        _ => None,
    }
}

fn map_history_search_key_to_action(key: KeyEvent) -> Option<UiAction> {
    match key.code {
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiAction::HistorySearchOlder)
        }
        KeyCode::Enter => Some(UiAction::HistorySearchAccept),
        KeyCode::Esc => Some(UiAction::HistorySearchCancel),
        KeyCode::Backspace => Some(UiAction::HistorySearchBackspace),
        KeyCode::Up => Some(UiAction::HistorySearchPrevious),
        KeyCode::Down => Some(UiAction::HistorySearchNext),
        KeyCode::Char(value) if terminal_shortcuts::is_text_input_key(&key) => {
            Some(UiAction::HistorySearchPush(value))
        }
        _ => None,
    }
}

fn map_shortcut_help_dock_key_to_action(key: KeyEvent) -> UiAction {
    match key.code {
        KeyCode::Esc if terminal_shortcuts::is_exit_key(&key) => UiAction::CloseShortcutHelpDock,
        KeyCode::Char('?') if key.modifiers == KeyModifiers::NONE => {
            UiAction::CloseShortcutHelpDock
        }
        _ => UiAction::Noop,
    }
}

fn map_tool_output_dock_key_to_action(key: KeyEvent) -> UiAction {
    match key.code {
        KeyCode::Esc if terminal_shortcuts::is_exit_key(&key) => UiAction::CloseToolOutputDock,
        KeyCode::Char('e') if key.modifiers == KeyModifiers::NONE => {
            UiAction::JumpToolOutputDockError
        }
        KeyCode::PageUp => UiAction::ScrollToolOutputDockUp,
        KeyCode::PageDown => UiAction::ScrollToolOutputDockDown,
        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
            UiAction::JumpToolOutputDockTop
        }
        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
            UiAction::JumpToolOutputDockBottom
        }
        _ => UiAction::Noop,
    }
}
