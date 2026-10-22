//! Agent TUI 运行中输入队列。

use std::collections::VecDeque;

use crate::r#loop::agent_tui::{
    AgentTuiQueuedAction, RuntimeError, TUI_HINT_WORKING, run_output::AgentTuiLifecyclePhase,
    state::AgentTuiState,
};
use crate::surface::cell::RenderStatusItem;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct InputQueueState {
    pub(super) queued_user_messages: VecDeque<QueuedUserMessage>,
    pub(super) queued_user_message_history_records: VecDeque<UserMessageHistoryRecord>,
    pub(super) user_turn_pending_start: bool,
    pub(super) pending_steers: VecDeque<PendingSteer>,
    pub(super) cancel_edit: CancelEditState,
    pub(super) visible_turn_activity: bool,
    pub(super) suppress_queue_autosend: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct UserMessage {
    pub(super) submission: super::AgentTuiSubmit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QueuedUserMessage {
    pub(super) user_message: UserMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingSteer {
    pub(super) user_message: UserMessage,
    pub(super) history_record: UserMessageHistoryRecord,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) enum UserMessageHistoryRecord {
    #[default]
    UserMessageText,
    Override(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct CancelEditState {
    prompt: Option<UserMessage>,
    history_record: UserMessageHistoryRecord,
    eligible: bool,
}

const QUEUED_PROMPT_PREVIEW_CHARS: usize = 48;
const QUEUED_INPUT_PREVIEW_MAX_ROWS: usize = 5;
const QUEUED_INPUT_OMITTED_ROW_COUNT: usize = 1;
const QUEUED_INPUT_EMPTY_HEIGHT: u16 = 0;
const QUEUED_INPUT_HEADER_HEIGHT: usize = 1;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn queue_draft_for_next_turn(&mut self) -> bool {
        if self.composer.draft.text().trim_start().starts_with('/') {
            let Some(submission) = self.take_submit_from_draft() else {
                return true;
            };
            self.enqueue_submission(submission, UserMessageHistoryRecord::UserMessageText);
            return true;
        }
        let Some((user_message, history_record)) = self.take_draft_user_message() else {
            return false;
        };
        self.enqueue_user_message(user_message, history_record);
        true
    }

    fn enqueue_submission(
        &mut self,
        submission: super::AgentTuiSubmit,
        history_record: UserMessageHistoryRecord,
    ) {
        self.enqueue_user_message(UserMessage { submission }, history_record);
    }

    fn enqueue_user_message(
        &mut self,
        user_message: UserMessage,
        history_record: UserMessageHistoryRecord,
    ) {
        self.composer
            .input_queue
            .queued_user_messages
            .push_back(QueuedUserMessage { user_message });
        self.composer
            .input_queue
            .queued_user_message_history_records
            .push_back(history_record);
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
        self.header.hint = TUI_HINT_WORKING;
    }

    pub(in crate::r#loop::agent_tui) fn take_queued_submit(
        &mut self,
    ) -> Option<super::AgentTuiSubmit> {
        self.take_queued_user_message_submit()
    }

    pub(in crate::r#loop::agent_tui) fn start_queued_submit(
        &mut self,
        submission: &super::AgentTuiSubmit,
    ) {
        self.composer.input_queue.user_turn_pending_start = true;
        self.start_runtime_submit(submission);
    }

    pub(in crate::r#loop::agent_tui) fn take_steer_submission_from_queue(
        &mut self,
    ) -> Option<(super::AgentTuiSubmit, UserMessageHistoryRecord)> {
        let sendable = self
            .composer
            .input_queue
            .queued_user_messages
            .front()
            .is_some_and(|queued| {
                matches!(
                    &queued.user_message.submission.input,
                    crate::r#loop::agent_tui::AgentTuiInputResult::Queued {
                        action: AgentTuiQueuedAction::Plain,
                        ..
                    }
                )
            });
        if !sendable {
            return None;
        }
        let queued = self.composer.input_queue.queued_user_messages.pop_front()?;
        let history_record = self
            .composer
            .input_queue
            .queued_user_message_history_records
            .pop_front()
            .unwrap_or(UserMessageHistoryRecord::UserMessageText);
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
        Some((queued.user_message.submission, history_record))
    }

    pub(in crate::r#loop::agent_tui) fn record_pending_steer(
        &mut self,
        submission: super::AgentTuiSubmit,
        history_record: UserMessageHistoryRecord,
    ) {
        self.composer
            .input_queue
            .pending_steers
            .push_back(PendingSteer {
                user_message: UserMessage { submission },
                history_record,
            });
        self.header.hint = TUI_HINT_WORKING;
    }

    pub(in crate::r#loop::agent_tui) fn commit_pending_steer(&mut self, content: String) {
        // 只认「排队中的 steer 被提交」这一件事。
        //
        // 事件流里的用户消息还包含回合的初始输入，而那条消息在本进程提交时就已经
        // 渲染过了；无 pending steer 时再补一条只会出现重复气泡，因此直接忽略。
        let Some(pending) = self.composer.input_queue.pending_steers.pop_front() else {
            return;
        };
        debug_assert_eq!(pending.user_message.submission.text(), content);
        let history_text = restore_text(&pending.user_message.submission, &pending.history_record);
        if history_text.trim().is_empty() {
            return;
        }
        self.record_input_history(&history_text);
        self.cells
            .transcript
            .push(crate::surface::cell::RenderCellRecord::user(history_text));
        self.header.hint = TUI_HINT_WORKING;
    }

    pub(in crate::r#loop::agent_tui) fn return_steer_to_queue(
        &mut self,
        submission: super::AgentTuiSubmit,
        history_record: UserMessageHistoryRecord,
    ) {
        self.composer
            .input_queue
            .queued_user_messages
            .push_front(QueuedUserMessage {
                user_message: UserMessage { submission },
            });
        self.composer
            .input_queue
            .queued_user_message_history_records
            .push_front(history_record);
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
        self.header.hint = TUI_HINT_WORKING;
    }

    pub(in crate::r#loop::agent_tui) fn requeue_pending_steers_for_next_turn(&mut self) {
        while let Some(pending) = self.composer.input_queue.pending_steers.pop_back() {
            self.composer
                .input_queue
                .queued_user_messages
                .push_front(QueuedUserMessage {
                    user_message: pending.user_message,
                });
            self.composer
                .input_queue
                .queued_user_message_history_records
                .push_front(pending.history_record);
        }
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
    }

    pub(in crate::r#loop::agent_tui) fn clear_pending_steers(&mut self) {
        self.composer.input_queue.pending_steers.clear();
    }

    pub(in crate::r#loop::agent_tui) fn push_steer_unsupported_notice(&mut self) {
        self.push_notice_status(
            "Steer",
            vec![RenderStatusItem {
                kind: "运行中输入".to_owned(),
                summary: "当前运行时不支持 steer，消息已保留在队列".to_owned(),
                phase: Some(AgentTuiLifecyclePhase::Notice),
                indicator: None,
                tool_name: None,
                duration_ms: None,
                turn_id: None,
                attempt_index: None,
            }],
        );
    }

    pub(in crate::r#loop::agent_tui) fn push_steer_rejected_notice(
        &mut self,
        error: Option<RuntimeError>,
    ) {
        let summary = error.map_or_else(
            || "当前 turn 拒绝 steer，消息已保留在队列".to_owned(),
            |error| error.message().to_owned(),
        );
        self.push_notice_status(
            "Steer",
            vec![RenderStatusItem {
                kind: "运行中输入".to_owned(),
                summary,
                phase: Some(AgentTuiLifecyclePhase::Notice),
                indicator: None,
                tool_name: None,
                duration_ms: None,
                turn_id: None,
                attempt_index: None,
            }],
        );
    }

    pub(in crate::r#loop::agent_tui) fn queued_submit_preview(&self) -> Option<String> {
        let input_queue = &self.composer.input_queue;
        if input_queue.pending_steers.is_empty() && input_queue.queued_user_messages.is_empty() {
            return None;
        }
        let mut lines = Vec::with_capacity(QUEUED_INPUT_PREVIEW_MAX_ROWS);
        for (index, pending) in input_queue.pending_steers.iter().enumerate() {
            let preview = message_preview(&pending.user_message, Some(&pending.history_record));
            lines.push(format!("等待注入 {}. {preview}", index + 1));
        }
        for (index, message) in input_queue.queued_user_messages.iter().enumerate() {
            let preview = message_preview(
                &message.user_message,
                input_queue.queued_user_message_history_records.get(index),
            );
            lines.push(format!("待发送 {}. {preview}", index + 1));
        }
        Some(compact_preview_lines(lines).join("\n"))
    }

    pub(in crate::r#loop::agent_tui) fn queued_submit_preview_height(&self) -> u16 {
        let Some(preview) = self.queued_submit_preview() else {
            return QUEUED_INPUT_EMPTY_HEIGHT;
        };
        u16::try_from(QUEUED_INPUT_HEADER_HEIGHT + preview.lines().count()).unwrap_or(u16::MAX)
    }

    pub(in crate::r#loop::agent_tui) fn has_queued_submit(&self) -> bool {
        self.composer.input_queue.has_follow_up_messages()
            || !self.composer.input_queue.pending_steers.is_empty()
    }

    pub(in crate::r#loop::agent_tui) fn has_queued_follow_up_message(&self) -> bool {
        !self.composer.input_queue.queued_user_messages.is_empty()
    }

    pub(in crate::r#loop::agent_tui) fn has_pending_steer(&self) -> bool {
        !self.composer.input_queue.pending_steers.is_empty()
    }

    pub(in crate::r#loop::agent_tui) fn restore_interrupt_input_to_draft(&mut self) -> bool {
        if !self.composer.input_queue.visible_turn_activity
            && self.composer.input_queue.pending_steers.is_empty()
            && !self.composer.input_queue.has_follow_up_messages()
            && self.composer.draft.text().trim().is_empty()
            && self.restore_cancel_edit_to_draft()
        {
            return true;
        }
        if !self.composer.input_queue.pending_steers.is_empty() {
            return self.restore_interrupted_messages_to_draft();
        }
        if self.restore_last_queued_message_to_draft() {
            return true;
        }
        self.restore_interrupted_messages_to_draft()
    }

    pub(in crate::r#loop::agent_tui) fn record_cancel_edit_candidate(
        &mut self,
        submission: &super::AgentTuiSubmit,
    ) {
        self.composer.input_queue.cancel_edit = CancelEditState {
            prompt: Some(UserMessage {
                submission: submission.clone(),
            }),
            history_record: UserMessageHistoryRecord::UserMessageText,
            eligible: true,
        };
        self.composer.input_queue.visible_turn_activity = false;
    }

    pub(in crate::r#loop::agent_tui) fn record_visible_turn_activity(&mut self) {
        self.composer.input_queue.visible_turn_activity = true;
        self.composer.input_queue.cancel_edit.eligible = false;
    }

    pub(in crate::r#loop::agent_tui) fn clear_cancel_edit_candidate(&mut self) {
        self.composer.input_queue.cancel_edit = CancelEditState::default();
        self.composer.input_queue.visible_turn_activity = false;
    }

    fn take_draft_user_message(&mut self) -> Option<(UserMessage, UserMessageHistoryRecord)> {
        let draft = self.composer.draft.text().trim().to_owned();
        if self.draft_blocks_unsupported_image(&draft) {
            return None;
        }
        if draft.is_empty() {
            return None;
        }
        let action = queued_action_for_draft(self.composer.draft.text());
        let history_record = history_record_for_draft(self.composer.draft.text(), &draft);
        let submission = self.submit_queued(draft, action, false);
        Some((UserMessage { submission }, history_record))
    }

    fn take_queued_user_message_submit(&mut self) -> Option<super::AgentTuiSubmit> {
        if self.composer.input_queue.suppress_queue_autosend {
            return None;
        }
        let queued = self.composer.input_queue.queued_user_messages.pop_front()?;
        let _history_record = self
            .composer
            .input_queue
            .queued_user_message_history_records
            .pop_front();
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
        let mut submission = queued.user_message.submission;
        if submission.overrides.session.is_none() {
            submission.overrides.session = self
                .header
                .overrides
                .session
                .clone()
                .or_else(|| self.header.session_summary.session_id.clone());
        }
        Some(submission)
    }

    fn restore_last_queued_message_to_draft(&mut self) -> bool {
        let Some(queued) = self.composer.input_queue.queued_user_messages.pop_back() else {
            return false;
        };
        let history_record = self
            .composer
            .input_queue
            .queued_user_message_history_records
            .pop_back()
            .unwrap_or(UserMessageHistoryRecord::UserMessageText);
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
        self.restore_user_message(queued.user_message, history_record, "已恢复到输入区");
        true
    }

    fn restore_cancel_edit_to_draft(&mut self) -> bool {
        if !self.composer.input_queue.cancel_edit.eligible {
            return false;
        }
        let Some(user_message) = self.composer.input_queue.cancel_edit.prompt.take() else {
            return false;
        };
        let history_record =
            std::mem::take(&mut self.composer.input_queue.cancel_edit.history_record);
        self.composer.input_queue.cancel_edit.eligible = false;
        self.restore_user_message(user_message, history_record, "已恢复中断输入");
        true
    }

    fn restore_interrupted_messages_to_draft(&mut self) -> bool {
        let mut restored = Vec::new();
        while let Some(pending) = self.composer.input_queue.pending_steers.pop_front() {
            restored.push((pending.user_message, pending.history_record));
        }
        while let Some(queued) = self.composer.input_queue.queued_user_messages.pop_front() {
            let history_record = self
                .composer
                .input_queue
                .queued_user_message_history_records
                .pop_front()
                .unwrap_or(UserMessageHistoryRecord::UserMessageText);
            restored.push((queued.user_message, history_record));
        }
        debug_assert_input_queue_lockstep(&self.composer.input_queue);
        if restored.is_empty() && self.composer.draft.text().trim().is_empty() {
            return false;
        }
        let mut parts = restored
            .into_iter()
            .map(|(message, record)| restore_text(&message.submission, &record))
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>();
        if !self.composer.draft.text().trim().is_empty() {
            parts.push(self.composer.draft.text().trim().to_owned());
        }
        if parts.is_empty() {
            return false;
        }
        self.replace_draft(parts.join("\n\n"));
        self.push_history_item(
            "输入",
            "恢复中断输入",
            "输入",
            Some(prompt_preview(self.composer.draft.text())),
        );
        self.header.hint = TUI_HINT_WORKING;
        true
    }

    fn restore_user_message(
        &mut self,
        user_message: UserMessage,
        history_record: UserMessageHistoryRecord,
        name: &'static str,
    ) {
        let text = restore_text(&user_message.submission, &history_record);
        self.restore_submission_to_draft(&text, &user_message.submission);
        self.push_history_item("输入", name, "待重新发送", Some(prompt_preview(&text)));
        self.header.hint = TUI_HINT_WORKING;
    }
}

impl InputQueueState {
    fn has_follow_up_messages(&self) -> bool {
        !self.queued_user_messages.is_empty()
    }
}

fn compact_preview_lines(lines: Vec<String>) -> Vec<String> {
    let total = lines.len();
    let visible_limit = queued_preview_visible_item_limit(total);
    let mut preview = lines.into_iter().take(visible_limit).collect::<Vec<_>>();
    let omitted = total.saturating_sub(visible_limit);
    if omitted == 0 {
        return preview;
    }
    preview.push(format!("另 {omitted} 条待处理"));
    preview
}

fn queued_preview_visible_item_limit(total: usize) -> usize {
    let body_capacity = QUEUED_INPUT_PREVIEW_MAX_ROWS.saturating_sub(QUEUED_INPUT_HEADER_HEIGHT);
    if total > body_capacity {
        body_capacity.saturating_sub(QUEUED_INPUT_OMITTED_ROW_COUNT)
    } else {
        total
    }
}

fn queued_action_for_draft(draft: &str) -> AgentTuiQueuedAction {
    let trimmed = draft.trim_start();
    if trimmed.starts_with('!') {
        AgentTuiQueuedAction::RunShell
    } else {
        AgentTuiQueuedAction::Plain
    }
}

fn history_record_for_draft(original: &str, submitted: &str) -> UserMessageHistoryRecord {
    let trimmed = original.trim();
    if trimmed != submitted && trimmed.starts_with('/') {
        UserMessageHistoryRecord::Override(trimmed.to_owned())
    } else {
        UserMessageHistoryRecord::UserMessageText
    }
}

fn message_preview(message: &UserMessage, record: Option<&UserMessageHistoryRecord>) -> String {
    prompt_preview(&restore_text(
        &message.submission,
        record.unwrap_or(&UserMessageHistoryRecord::UserMessageText),
    ))
}

fn restore_text(submission: &super::AgentTuiSubmit, record: &UserMessageHistoryRecord) -> String {
    match record {
        UserMessageHistoryRecord::UserMessageText => submission.text().to_owned(),
        UserMessageHistoryRecord::Override(text) => text.clone(),
    }
}

fn prompt_preview(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = normalized
        .chars()
        .take(QUEUED_PROMPT_PREVIEW_CHARS)
        .collect::<String>();
    if normalized.chars().count() > QUEUED_PROMPT_PREVIEW_CHARS {
        preview.push('…');
    }
    preview
}

fn debug_assert_input_queue_lockstep(state: &InputQueueState) {
    debug_assert_eq!(
        state.queued_user_messages.len(),
        state.queued_user_message_history_records.len()
    );
}
