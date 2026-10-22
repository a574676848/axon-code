//! Agent TUI 本地 action reducer 实现。

use std::time::Instant;

use crate::{
    app_shell::composer_dock::DockKind,
    r#loop::agent_tui::state::AgentTuiState,
    ui_workflow::{
        action::UiAction,
        reducer::{UiActionOutcome, UiActionReducer, UiReducer, UiSideEffect},
    },
};

impl UiActionReducer<AgentTuiState> for UiReducer {
    fn reduce(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
        match action {
            UiAction::Noop | UiAction::Exit => UiActionOutcome::ignored(),
            UiAction::PasteText(_)
            | UiAction::PasteFromSystemClipboard
            | UiAction::PasteImageFromSystemClipboard
            | UiAction::OpenExternalEditor
            | UiAction::RestoreInterruptInput
            | UiAction::ClearDraft => reduce_core_action(state, action),
            UiAction::OpenSelectedToolOutputDock
            | UiAction::ToggleShortcutHelpDock
            | UiAction::CloseTopDock
            | UiAction::CloseShortcutHelpDock
            | UiAction::CloseToolOutputDock
            | UiAction::ScrollActiveSurfaceUp
            | UiAction::ScrollActiveSurfaceDown
            | UiAction::BottomViewKey(_) => reduce_dock_action(state, action),
            UiAction::ComposerPopupPrevious
            | UiAction::ComposerPopupNext
            | UiAction::ComposerPopupAccept
            | UiAction::ComposerPopupClose
            | UiAction::CommandSuggestionPrevious
            | UiAction::CommandSuggestionNext
            | UiAction::CommandSuggestionAccept
            | UiAction::CommandSuggestionAcceptForEnter
            | UiAction::CompleteCommandTrigger => reduce_popup_action(state, action),
            UiAction::HistoryPrevious
            | UiAction::HistoryNext
            | UiAction::MoveCursorWordLeft
            | UiAction::MoveCursorWordRight
            | UiAction::MoveCursorLeft
            | UiAction::MoveCursorRight
            | UiAction::MoveCursorHome
            | UiAction::MoveCursorEnd
            | UiAction::MoveCursorUp
            | UiAction::MoveCursorDown
            | UiAction::KillWordBeforeCursor
            | UiAction::KillWordAfterCursor
            | UiAction::Backspace
            | UiAction::DeleteChar
            | UiAction::KillToLineEnd
            | UiAction::KillToLineStart
            | UiAction::InsertSpace
            | UiAction::InsertPlainChar { .. }
            | UiAction::InsertNewline => reduce_edit_action(state, action),
            UiAction::SubmitCurrentDraft { .. }
            | UiAction::SubmitSteer { .. }
            | UiAction::InterruptRuntime => reduce_runtime_control_action(state, action),
            UiAction::ToggleHistorySearch
            | UiAction::HistorySearchOlder
            | UiAction::HistorySearchAccept
            | UiAction::HistorySearchCancel
            | UiAction::HistorySearchBackspace
            | UiAction::HistorySearchPrevious
            | UiAction::HistorySearchNext
            | UiAction::HistorySearchPush(_) => reduce_history_search_action(state, action),
            _ => reduce_navigation_action(state, action),
        }
    }
}

fn reduce_navigation_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::PageTranscriptViewUp
        | UiAction::PageTranscriptViewDown
        | UiAction::JumpTranscriptViewTop
        | UiAction::JumpTranscriptViewBottom => reduce_transcript_view_action(state, action),
        UiAction::VisibleTextSelectionBegin { .. }
        | UiAction::VisibleTextSelectionDrag { .. }
        | UiAction::VisibleTextSelectionRelease { .. } => reduce_viewport_action(state, action),
        UiAction::ScrollToolOutputDockUp
        | UiAction::ScrollToolOutputDockDown
        | UiAction::JumpToolOutputDockTop
        | UiAction::JumpToolOutputDockBottom
        | UiAction::JumpToolOutputDockError => reduce_tool_output_action(state, action),
        _ => UiActionOutcome::ignored(),
    }
}

fn reduce_core_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::PasteText(text) => {
            state.flush_paste_burst_before_modified_input();
            state.paste(&text);
            UiActionOutcome::redraw()
        }
        UiAction::PasteFromSystemClipboard => {
            state.paste_from_system_clipboard();
            UiActionOutcome::redraw()
        }
        UiAction::PasteImageFromSystemClipboard => {
            state.paste_image_from_system_clipboard();
            UiActionOutcome::redraw()
        }
        UiAction::OpenExternalEditor => {
            UiActionOutcome::side_effect(UiSideEffect::OpenExternalEditor)
        }
        UiAction::RestoreInterruptInput => redraw_if(state.restore_interrupt_input_to_draft()),
        UiAction::ClearDraft => {
            state.clear_draft();
            UiActionOutcome::redraw()
        }
        _ => unreachable!("非 core action 不应进入 reduce_core_action"),
    }
}

fn reduce_dock_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::OpenSelectedToolOutputDock => redraw_if(state.open_selected_tool_output_dock()),
        UiAction::ToggleShortcutHelpDock => {
            state.toggle_shortcut_help_dock();
            UiActionOutcome::redraw()
        }
        UiAction::CloseTopDock => {
            state.close_top_dock();
            UiActionOutcome::redraw()
        }
        UiAction::CloseShortcutHelpDock => {
            state.close_shortcut_help_dock();
            UiActionOutcome::redraw()
        }
        UiAction::CloseToolOutputDock => {
            state.close_tool_output_viewer_dock();
            UiActionOutcome::redraw()
        }
        UiAction::ScrollActiveSurfaceUp => {
            scroll_active_surface_up(state);
            UiActionOutcome::redraw()
        }
        UiAction::ScrollActiveSurfaceDown => {
            scroll_active_surface_down(state);
            UiActionOutcome::redraw()
        }
        UiAction::BottomViewKey(key) => match state.handle_bottom_view_key(key) {
            // 待决问答作答不是一轮新任务：它不重开转录单元、不重置 turn 台账，
            // 只把作答发往统一答案入口。
            Some(submission) if submission.is_question_answer() => {
                state.bridge.pending_runtime_submit = Some(submission);
                UiActionOutcome::side_effect(UiSideEffect::AnswerQuestion)
            }
            Some(submission) => {
                state.start_runtime_submit(&submission);
                state.bridge.pending_runtime_submit = Some(submission);
                UiActionOutcome::side_effect(UiSideEffect::RuntimeSubmit)
            }
            None => UiActionOutcome::redraw(),
        },
        _ => unreachable!("非 dock action 不应进入 reduce_dock_action"),
    }
}

fn reduce_popup_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::ComposerPopupPrevious => {
            state.select_previous_composer_popup_item();
            UiActionOutcome::redraw()
        }
        UiAction::ComposerPopupNext => {
            state.select_next_composer_popup_item();
            UiActionOutcome::redraw()
        }
        UiAction::ComposerPopupAccept => redraw_if(state.accept_selected_composer_popup_item()),
        UiAction::ComposerPopupClose => {
            state.close_composer_popup();
            UiActionOutcome::redraw()
        }
        UiAction::CommandSuggestionPrevious => {
            state.select_previous_command_suggestion();
            UiActionOutcome::redraw()
        }
        UiAction::CommandSuggestionNext => {
            state.select_next_command_suggestion();
            UiActionOutcome::redraw()
        }
        UiAction::CommandSuggestionAccept => redraw_if(state.accept_selected_command_suggestion()),
        UiAction::CommandSuggestionAcceptForEnter => {
            reduce_command_suggestion_accept_for_enter(state)
        }
        UiAction::CompleteCommandTrigger => {
            state.complete_command_trigger();
            UiActionOutcome::redraw()
        }
        _ => unreachable!("非 popup action 不应进入 reduce_popup_action"),
    }
}

fn reduce_command_suggestion_accept_for_enter(state: &mut AgentTuiState) -> UiActionOutcome {
    if !state.accept_selected_command_suggestion_for_enter() {
        return UiActionOutcome::redraw();
    }
    if state.is_working() {
        if state.queue_draft_for_next_turn() {
            return UiActionOutcome::side_effect(UiSideEffect::RuntimeSteer);
        }
        return UiActionOutcome::redraw();
    }
    reduce_submit_current_draft(state, Instant::now())
}

fn reduce_edit_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::HistoryPrevious => {
            state.history_previous();
            UiActionOutcome::redraw()
        }
        UiAction::HistoryNext => {
            state.history_next();
            UiActionOutcome::redraw()
        }
        UiAction::MoveCursorWordLeft => {
            redraw_after_flush(state, AgentTuiState::move_cursor_word_left)
        }
        UiAction::MoveCursorWordRight => {
            redraw_after_flush(state, AgentTuiState::move_cursor_word_right)
        }
        UiAction::MoveCursorLeft => redraw_after_flush(state, AgentTuiState::move_cursor_left),
        UiAction::MoveCursorRight => redraw_after_flush(state, AgentTuiState::move_cursor_right),
        UiAction::MoveCursorHome => redraw_after_flush(state, AgentTuiState::move_cursor_home),
        UiAction::MoveCursorEnd => redraw_after_flush(state, AgentTuiState::move_cursor_end),
        UiAction::MoveCursorUp => redraw_after_flush(state, AgentTuiState::move_cursor_up),
        UiAction::MoveCursorDown => redraw_after_flush(state, AgentTuiState::move_cursor_down),
        UiAction::KillWordBeforeCursor => {
            redraw_after_flush(state, AgentTuiState::kill_word_before_cursor)
        }
        UiAction::KillWordAfterCursor => {
            redraw_after_flush(state, AgentTuiState::kill_word_after_cursor)
        }
        UiAction::Backspace => redraw_after_flush(state, AgentTuiState::backspace),
        UiAction::DeleteChar => redraw_after_flush(state, AgentTuiState::delete_char),
        UiAction::KillToLineEnd => redraw_after_flush(state, AgentTuiState::kill_to_line_end),
        UiAction::KillToLineStart => redraw_after_flush(state, AgentTuiState::kill_to_line_start),
        UiAction::InsertSpace => {
            state.flush_paste_burst_before_modified_input();
            state.insert_str(" ");
            UiActionOutcome::redraw()
        }
        UiAction::InsertPlainChar { value, now } => {
            state.handle_plain_char(value, now);
            UiActionOutcome::redraw()
        }
        UiAction::InsertNewline => {
            state.insert_newline();
            UiActionOutcome::redraw()
        }
        _ => unreachable!("非 edit action 不应进入 reduce_edit_action"),
    }
}

fn reduce_runtime_control_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::SubmitCurrentDraft { now } => reduce_submit_current_draft(state, now),
        UiAction::SubmitSteer { .. } => UiActionOutcome::side_effect(UiSideEffect::RuntimeSteer),
        UiAction::InterruptRuntime => {
            state.requeue_pending_steers_for_next_turn();
            UiActionOutcome::side_effect(UiSideEffect::RuntimeInterrupt {
                reason: "用户中断当前任务",
            })
        }
        _ => unreachable!("非 runtime control action 不应进入 reduce_runtime_control_action"),
    }
}

fn reduce_history_search_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::ToggleHistorySearch => state.toggle_history_search(),
        UiAction::HistorySearchOlder => state.continue_history_search_older(),
        UiAction::HistorySearchAccept => state.accept_history_search(),
        UiAction::HistorySearchCancel => state.cancel_history_search(),
        UiAction::HistorySearchBackspace => state.backspace_history_search(),
        UiAction::HistorySearchPrevious => state.history_search_previous(),
        UiAction::HistorySearchNext => state.history_search_next(),
        UiAction::HistorySearchPush(value) => state.push_history_search(value),
        _ => unreachable!("非 history search action 不应进入 reduce_history_search_action"),
    }
    UiActionOutcome::redraw()
}

fn reduce_transcript_view_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::PageTranscriptViewUp => state.page_transcript_view_up(),
        UiAction::PageTranscriptViewDown => state.page_transcript_view_down(),
        UiAction::JumpTranscriptViewTop => state.jump_transcript_view_top(),
        UiAction::JumpTranscriptViewBottom => state.jump_transcript_view_bottom(),
        _ => unreachable!("非 transcript view action 不应进入 reduce_transcript_view_action"),
    }
    UiActionOutcome::redraw()
}

fn reduce_viewport_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::VisibleTextSelectionBegin { row, column } => {
            state.visible_text_selection.begin(row, column);
            let _ = column;
            UiActionOutcome::redraw()
        }
        UiAction::VisibleTextSelectionDrag { row, column } => {
            let changed = state.visible_text_selection.update(row, column);
            redraw_if(changed)
        }
        UiAction::VisibleTextSelectionRelease { row, column, now } => {
            state.visible_text_selection.update(row, column);
            if let Some((click_row, click_column)) =
                state.visible_text_selection.take_click_position()
            {
                if state
                    .visible_text_selection
                    .select_content_run_on_double_click(click_row, click_column, now)
                {
                    state.visible_text_selection.retain_active_selection();
                    return redraw_if(state.copy_visible_text_selection_to_clipboard());
                }
                return redraw_if(state.toggle_transcript_expandable_at(click_row));
            }
            redraw_if(state.copy_visible_text_selection_to_clipboard())
        }
        _ => unreachable!("非 viewport action 不应进入 reduce_viewport_action"),
    }
}

fn reduce_tool_output_action(state: &mut AgentTuiState, action: UiAction) -> UiActionOutcome {
    match action {
        UiAction::ScrollToolOutputDockUp => state.scroll_tool_output_dock_up(),
        UiAction::ScrollToolOutputDockDown => state.scroll_tool_output_dock_down(),
        UiAction::JumpToolOutputDockTop => state.jump_tool_output_dock_top(),
        UiAction::JumpToolOutputDockBottom => state.jump_tool_output_dock_bottom(),
        UiAction::JumpToolOutputDockError => return redraw_if(state.jump_tool_output_dock_error()),
        _ => unreachable!("非 tool output action 不应进入 reduce_tool_output_action"),
    }
    UiActionOutcome::redraw()
}

fn reduce_submit_current_draft(state: &mut AgentTuiState, now: Instant) -> UiActionOutcome {
    if state.paste_burst_insert_newline_if_needed(now) {
        return UiActionOutcome::redraw();
    }
    if state.composer.draft.text().is_empty() && state.open_selected_tool_output_dock() {
        return UiActionOutcome::redraw();
    }
    state.flush_paste_burst_before_modified_input();
    if state.is_working() {
        if state.queue_draft_for_next_turn() {
            return UiActionOutcome::side_effect(UiSideEffect::RuntimeSteer);
        }
        return UiActionOutcome::redraw();
    }
    if let Some(submission) = state.begin_submit() {
        debug_assert!(state.bridge.pending_runtime_submit.is_some());
        drop(submission);
        return UiActionOutcome::side_effect(UiSideEffect::RuntimeSubmit);
    }
    UiActionOutcome::redraw()
}

fn redraw_after_flush(state: &mut AgentTuiState, apply: fn(&mut AgentTuiState)) -> UiActionOutcome {
    state.flush_paste_burst_before_modified_input();
    apply(state);
    UiActionOutcome::redraw()
}

const fn redraw_if(changed: bool) -> UiActionOutcome {
    if changed {
        UiActionOutcome::redraw()
    } else {
        UiActionOutcome::ignored()
    }
}

fn scroll_active_surface_up(state: &mut AgentTuiState) {
    match state.active_dock_kind() {
        Some(DockKind::ToolOutputViewer) => state.scroll_tool_output_dock_mouse_up(),
        _ => state.scroll_transcript_view_up(),
    }
}

fn scroll_active_surface_down(state: &mut AgentTuiState) {
    match state.active_dock_kind() {
        Some(DockKind::ToolOutputViewer) => state.scroll_tool_output_dock_mouse_down(),
        _ => state.scroll_transcript_view_down(),
    }
}
