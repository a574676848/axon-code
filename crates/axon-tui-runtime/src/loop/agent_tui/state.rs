//! TUI 状态管理 - Application Layer。
//!
//! 管理 TUI 的所有状态（输入、历史、附件、滚动等），不包含渲染逻辑。

#[path = "state/active_view_state.rs"]
mod active_view_state;
#[cfg(test)]
#[path = "state/active_view_state_tests.rs"]
mod active_view_state_tests;
#[path = "state/bottom_views.rs"]
mod bottom_views;
#[path = "state/cell_history_gc.rs"]
mod cell_history_gc;
#[path = "state/clipboard.rs"]
mod clipboard_state;
#[path = "state/command_handlers.rs"]
mod command_handlers;
#[cfg(test)]
#[path = "state/command_handlers_tests.rs"]
mod command_handlers_tests;
#[path = "state/composer_popup_state.rs"]
mod composer_popup_state;
#[path = "state/consolidation.rs"]
mod consolidation;
#[path = "state/core_state.rs"]
mod core_state;
#[path = "state/docks.rs"]
mod docks;
#[path = "state/draft_editing.rs"]
mod draft_editing;
#[path = "state/frame_render_signature.rs"]
mod frame_render_signature;
#[path = "state/history_navigation.rs"]
mod history_navigation;
#[path = "state/invariants.rs"]
mod invariants;
#[cfg(test)]
#[path = "state/mcp_command_tests.rs"]
mod mcp_command_tests;
#[path = "state/queued_input.rs"]
mod queued_input;
#[path = "state/resume_replay.rs"]
mod resume_replay;
#[path = "state/runtime_assistant_content.rs"]
mod runtime_assistant_content;
#[cfg(test)]
#[path = "state/runtime_assistant_stream_tests.rs"]
mod runtime_assistant_stream_tests;
#[path = "state/runtime_diff_snapshot.rs"]
mod runtime_diff_snapshot;
#[cfg(test)]
#[path = "state/runtime_diff_tests.rs"]
mod runtime_diff_tests;
#[path = "state/runtime_error_projection.rs"]
mod runtime_error_projection;
#[cfg(test)]
#[path = "state/runtime_exec_diff_lifecycle_tests.rs"]
mod runtime_exec_diff_lifecycle_tests;
#[cfg(test)]
#[path = "state/runtime_exec_lifecycle_tests.rs"]
mod runtime_exec_lifecycle_tests;
#[cfg(test)]
#[path = "state/runtime_exec_tick_tests.rs"]
mod runtime_exec_tick_tests;
#[path = "state/runtime_flow.rs"]
mod runtime_flow;
#[path = "state/runtime_flow_cell_access.rs"]
mod runtime_flow_cell_access;
#[path = "state/runtime_flow_dispatch.rs"]
mod runtime_flow_dispatch;
#[path = "state/runtime_flow_helpers.rs"]
mod runtime_flow_helpers;
#[cfg(test)]
#[path = "state/runtime_flow_test_support.rs"]
mod runtime_flow_test_support;
#[cfg(test)]
#[path = "state/runtime_flow_tests.rs"]
mod runtime_flow_tests;
#[path = "state/runtime_history_lifecycle.rs"]
mod runtime_history_lifecycle;
#[path = "state/runtime_hook_lifecycle.rs"]
mod runtime_hook_lifecycle;
#[cfg(test)]
#[path = "state/runtime_hook_tests.rs"]
mod runtime_hook_tests;
#[cfg(test)]
#[path = "state/runtime_mcp_history_lifecycle_tests.rs"]
mod runtime_mcp_history_lifecycle_tests;
#[cfg(test)]
#[path = "state/runtime_memory_projection_tests.rs"]
mod runtime_memory_projection_tests;
#[cfg(test)]
#[path = "state/runtime_output_projection_tests.rs"]
mod runtime_output_projection_tests;
#[path = "state/runtime_performance.rs"]
pub(in crate::r#loop::agent_tui) mod runtime_performance;
#[path = "state/runtime_plan_review_lifecycle.rs"]
mod runtime_plan_review_lifecycle;
#[path = "state/runtime_projection_policy.rs"]
mod runtime_projection_policy;
#[path = "state/runtime_question_lifecycle.rs"]
mod runtime_question_lifecycle;
#[cfg(test)]
#[path = "state/runtime_question_tests.rs"]
mod runtime_question_tests;
#[path = "state/runtime_reasoning_lifecycle.rs"]
mod runtime_reasoning_lifecycle;
#[cfg(test)]
#[path = "state/runtime_reasoning_tests.rs"]
mod runtime_reasoning_tests;
#[cfg(test)]
#[path = "state/runtime_resume_snapshot_tests.rs"]
mod runtime_resume_snapshot_tests;
#[path = "state/runtime_status_lifecycle.rs"]
mod runtime_status_lifecycle;
#[path = "state/runtime_submission.rs"]
mod runtime_submission;
#[path = "state/runtime_tick.rs"]
mod runtime_tick;
#[path = "state/runtime_turn_summary.rs"]
mod runtime_turn_summary;
#[cfg(test)]
#[path = "state/runtime_usage_accounting_tests.rs"]
mod runtime_usage_accounting_tests;
#[path = "state/runtime_usage_lifecycle.rs"]
mod runtime_usage_lifecycle;
#[cfg(test)]
#[path = "state/runtime_watcher_tests.rs"]
mod runtime_watcher_tests;
#[path = "state/runtime_wire_policy.rs"]
mod runtime_wire_policy;
#[path = "state/runtime_wire_projection.rs"]
mod runtime_wire_projection;
#[path = "state/session_handoff.rs"]
mod session_handoff;
#[path = "state/state_dump.rs"]
mod state_dump;
#[path = "state/subagent_activity.rs"]
pub(in crate::r#loop::agent_tui) mod subagent_activity;
#[cfg(test)]
#[path = "state/subagent_activity_tests.rs"]
mod subagent_activity_tests;
#[path = "state/suggestions.rs"]
mod suggestions;
#[path = "state/telemetry.rs"]
mod telemetry;
#[path = "state/terminal_session_projection.rs"]
mod terminal_session_projection;
#[cfg(test)]
#[path = "state/terminal_session_projection_tests.rs"]
mod terminal_session_projection_tests;
#[path = "state/title_context.rs"]
mod title_context;
#[path = "state/toast.rs"]
mod toast;
#[path = "state/ui_mode.rs"]
mod ui_mode;
#[cfg(test)]
#[path = "state/ui_mode_tests.rs"]
mod ui_mode_tests;
#[path = "state/viewport_state.rs"]
pub(crate) mod viewport_state;
#[cfg(test)]
#[path = "state/viewport_state_tests.rs"]
mod viewport_state_tests;
#[path = "state/visible_text_selection.rs"]
pub(crate) mod visible_text_selection;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{
    AgentTuiRunOverrides, AgentTuiSkillStateChange, AgentTuiTokenUsage, RuntimeError,
    TUI_HINT_IDLE,
    attachments::{infer_attachment_kind, validated_attachment},
    clipboard,
    error_projection::local_error_cell,
    input::AgentTuiInputResult,
    output_dock::ToolOutputDock,
    runtime_bridge::RuntimeBridge,
};
use crate::surface::{
    agent_output::normalize_agent_output,
    cell::{
        ROLE_ERROR, RenderCellRecord, RenderCommandCell, RenderStatusCell, RenderStatusItem,
        STATUS_KIND_INTERRUPT_REQUESTED,
    },
    cell_event_data::{AgentRenderHistoryCell, AgentRenderHistoryItem},
    cell_store::CellStore,
};
use crate::ui_workflow::composer_state::ComposerState;
use crate::{AgentTuiSessionSummary, AgentTuiSettingsChange, app_shell::header_state::HeaderState};
pub(in crate::r#loop::agent_tui) use draft_editing::composer_prompt_width;
pub(crate) use queued_input::InputQueueState;
use terminal_session_projection::TerminalSessionProjectionState;
pub(crate) use toast::{ToastKind, ToastState};
pub(crate) use viewport_state::ViewportState;
pub(crate) use visible_text_selection::VisibleTextSelectionState;

/// TUI 状态。
#[derive(Debug, Clone)]
pub(super) struct AgentTuiState {
    pub(in crate::r#loop::agent_tui) header: HeaderState,
    pub(in crate::r#loop::agent_tui) viewport: ViewportState,
    pub(in crate::r#loop::agent_tui) composer: ComposerState,
    pub(in crate::r#loop::agent_tui) cells: CellStore,
    pub(in crate::r#loop::agent_tui) bridge: RuntimeBridge,
    pub(in crate::r#loop::agent_tui) visible_text_selection: VisibleTextSelectionState,
    pub(in crate::r#loop::agent_tui) toast: ToastState,
    clipboard: clipboard::ClipboardBackend,
    // 状态克隆共享临时目录生命周期，最后一个测试状态释放时自动清理。
    #[cfg(test)]
    test_runtime_root: Option<std::sync::Arc<axon_test_support::TestTempDir>>,
    terminal_session_projection: TerminalSessionProjectionState,
}

const HOOK_REVEAL_DELAY: Duration = Duration::from_millis(300);
const HOOK_QUIET_SUCCESS_LINGER: Duration = Duration::from_millis(600);
const HISTORY_SEARCH_HINT: &str = "Ctrl+R 继续查更旧 · Enter 采用 · Esc 退出 · ↑/↓ 切换";
const LOCAL_COMMAND_STATUS_PART_ID: &str = "command:local:status";
const LOCAL_NOTICE_STATUS_PART_ID: &str = "status:local:notice";
const LOCAL_OUTPUT_ERROR_PART_ID: &str = "error:local:output";
const LOCAL_ASSISTANT_OUTPUT_PART_ID: &str = "assistant:local:output";
const LOCAL_PUSH_ERROR_PART_ID: &str = "error:local:push";
/// 完成通知在 transcript 中的展示前缀，用于与主 turn 回复区分。
const COMPLETION_NOTICE_PREFIX: &str = "后台完成通知 · ";
/// 完成通知收件箱失败提示标题。
const COMPLETION_INBOX_FAILURE_TITLE: &str = "完成通知收件箱不可用：";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentRuntimeStatus {
    Ready,
    Working,
}

impl AgentRuntimeStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Working => "working",
        }
    }
}

/// Agent TUI 提交给真实运行时的任务载荷。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiSubmit {
    /// 用户输入解析后的结构化结果。
    pub input: AgentTuiInputResult,
    /// 当前 TUI 会话覆盖的运行参数。
    pub overrides: AgentTuiRunOverrides,
    /// 提交发生时的当前工作目录。
    pub cwd: PathBuf,
    /// 待决问答的作答载荷；普通任务提交为 `None`。
    pub question_answer: Option<AgentTuiQuestionAnswer>,
}

/// TUI 待决问答的作答载荷。
///
/// 作答不是一轮新任务：它必须回到提出问题的那条车道并交给统一答案入口，
/// 因此单独携带车道身份与调用标识，而不是被拼成一段用户正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTuiQuestionAnswer {
    /// 问题所属会话车道身份。
    pub lane_id: String,
    /// 问题调用 ID（提问工具的 tool_call_id）。
    pub call_id: String,
    /// 用户作答条目。
    pub answers: Vec<axon_domain::UserAnswerItem>,
}

impl AgentTuiSubmit {
    /// 提交给真实运行时的文本。
    #[must_use]
    pub fn text(&self) -> &str {
        self.input.text()
    }

    /// 当前提交携带的附件路径。
    #[must_use]
    pub fn file_paths(&self) -> Vec<PathBuf> {
        self.input.file_paths()
    }

    /// 当前提交是否为待决问答作答。
    #[must_use]
    pub const fn is_question_answer(&self) -> bool {
        self.question_answer.is_some()
    }
}

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn settle_projection_once(
        &mut self,
        part_id: &str,
        kind: &str,
    ) -> bool {
        self.bridge
            .turn_ledger
            .settle_projection_once(part_id, kind)
    }

    pub(in crate::r#loop::agent_tui) fn projection_is_settled(
        &self,
        part_id: &str,
        kind: &str,
    ) -> bool {
        self.bridge.turn_ledger.projection_is_settled(part_id, kind)
    }
    pub(super) fn paste(&mut self, value: &str) {
        let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
        if self.try_attach_pasted_path(&normalized) {
            return;
        }
        self.insert_pasted_text(normalized);
    }

    pub(in crate::r#loop::agent_tui) fn push_toast(
        &mut self,
        text: impl Into<String>,
        kind: ToastKind,
        now: Instant,
    ) {
        self.toast.show(text, kind, now);
    }

    pub(in crate::r#loop::agent_tui) fn current_toast(
        &self,
        now: Instant,
    ) -> Option<&toast::ToastMessage> {
        self.toast.current(now)
    }

    pub(super) fn paste_burst_insert_newline_if_needed(&mut self, now: Instant) -> bool {
        if self.composer.draft.append_newline_if_paste_active(now) {
            self.fold_completed_clipboard_large_paste();
            return true;
        }
        let inferred_from_clipboard = self.clipboard.read_text().is_ok_and(|clipboard_text| {
            self.composer
                .draft
                .observe_clipboard_multiline_prefix(&clipboard_text)
        });
        if !self.composer.draft.paste_newline_should_insert(now) && !inferred_from_clipboard {
            return false;
        }
        self.flush_paste_burst_before_modified_input();
        self.insert_str("\n");
        self.fold_completed_clipboard_large_paste();
        true
    }

    fn push_agent_output(&mut self, role: &'static str, message: &str) {
        let trimmed = message.trim();
        if trimmed.is_empty() {
            return;
        }
        let fallback = if role == ROLE_ERROR {
            "任务失败，请检查上方错误。"
        } else {
            ""
        };
        for line in normalize_agent_output(trimmed, fallback) {
            self.cells.transcript.push(if role == ROLE_ERROR {
                RenderCellRecord::error_with_data(local_error_cell(
                    LOCAL_OUTPUT_ERROR_PART_ID,
                    "任务失败",
                    line,
                    None,
                ))
            } else {
                RenderCellRecord::agent_message(LOCAL_ASSISTANT_OUTPUT_PART_ID, line)
            });
        }
    }

    pub(super) fn clear(&mut self) {
        self.cells.transcript.clear();
        self.cells
            .transcript
            .push(RenderCellRecord::system("已清空当前会话视图。"));
        self.viewport.reset_transcript_view_to_bottom();
        self.composer.submitted_prompt = None;
        self.clear_active_transcript_lines();
        self.cells.active_runtime_cells.clear();
        self.terminal_session_projection.clear();
        self.bridge.subagent_activity.clear_and_suppress();
        self.viewport.clear_runtime_selection();
        self.close_all_docks();
        self.viewport.tool_output_dock = ToolOutputDock::default();
        self.cells.tool_outputs_by_part_id.clear();
    }

    pub(super) fn push_editor_error(&mut self, message: &str) {
        self.push_agent_output(ROLE_ERROR, message);
        self.header.hint = TUI_HINT_IDLE;
    }

    pub(super) fn should_exit(&self) -> bool {
        self.bridge.should_exit
    }

    pub(super) fn session_usage(&self) -> AgentTuiTokenUsage {
        self.header.session_usage
    }

    pub(super) fn last_turn_usage(&self) -> AgentTuiTokenUsage {
        self.header.last_turn_usage
    }

    pub(super) fn display_elapsed(&self, now: Instant) -> Option<Duration> {
        self.bridge.live_status.display_elapsed(now)
    }

    pub(super) fn session_summary(&self) -> AgentTuiSessionSummary {
        self.header.session_summary.clone()
    }

    pub(super) fn models(&self) -> &[super::AgentTuiModelCatalogItem] {
        &self.header.catalog.models
    }

    pub(super) fn take_settings_changes(&mut self) -> Vec<AgentTuiSettingsChange> {
        std::mem::take(&mut self.header.settings_changes)
    }

    pub(super) fn record_settings_change(&mut self) {
        self.header
            .settings_changes
            .push(AgentTuiSettingsChange::from_overrides(
                &self.header.overrides,
            ));
    }

    pub(super) fn record_skill_settings_change(
        &mut self,
        skill_states: Vec<AgentTuiSkillStateChange>,
    ) {
        self.header
            .settings_changes
            .push(AgentTuiSettingsChange::from_skill_states(skill_states));
    }

    pub(in crate::r#loop::agent_tui) fn interrupt_requested(&self) -> bool {
        self.bridge.interrupt_requested
    }

    pub(in crate::r#loop::agent_tui) fn request_interrupt_current_task(&mut self) {
        if self.bridge.interrupt_requested {
            return;
        }
        self.bridge.interrupt_requested = true;
        self.flush_live_assistant_output();
        self.bridge.runtime_status = AgentRuntimeStatus::Ready;
        self.bridge.generation_performance.turn_terminal = true;
        self.advance_terminal_title_lifecycle();
        self.composer.input_queue.user_turn_pending_start = false;
        self.bridge.live_status.clear();
        self.bridge.provider_activity.clear();
        self.bridge.subagent_activity.clear_and_suppress();
        self.header.hint = TUI_HINT_IDLE;
        if let Some(active_plan) = self.take_live_plan_line() {
            self.settle_active_plan_cell(active_plan);
        }
        self.clear_active_transcript_lines();
        self.cells.active_runtime_cells.clear();
        self.push_notice_status(
            "中断",
            vec![RenderStatusItem {
                kind: STATUS_KIND_INTERRUPT_REQUESTED.to_owned(),
                summary: "已停止，可继续输入".to_owned(),
                phase: Some(crate::AgentTuiLifecyclePhase::Notice),
                indicator: None,
                tool_name: None,
                duration_ms: None,
                turn_id: None,
                attempt_index: None,
            }],
        );
    }

    pub(in crate::r#loop::agent_tui) fn push_runtime_idle_notice(&mut self, idle_for: Duration) {
        let idle_minutes = idle_for.as_secs() / 60;
        self.push_notice_status(
            "运行中",
            vec![RenderStatusItem {
                kind: "等待".to_owned(),
                summary: format!(
                    "Runtime 已 {} 分钟无新进展，任务仍在运行；可继续等待或按 Esc 中断",
                    idle_minutes.max(1)
                ),
                phase: Some(crate::AgentTuiLifecyclePhase::Notice),
                indicator: None,
                tool_name: None,
                duration_ms: Some(idle_for.as_millis().try_into().unwrap_or(u64::MAX)),
                turn_id: None,
                attempt_index: None,
            }],
        );
    }

    pub(super) fn composer_popup_open(&self) -> bool {
        self.composer
            .composer_popup
            .as_ref()
            .is_some_and(|popup| !popup.is_empty())
    }

    fn try_attach_pasted_path(&mut self, value: &str) -> bool {
        let trimmed = value.trim().trim_matches('"').trim_matches('\'');
        if trimmed.lines().count() != 1 || trimmed.is_empty() {
            return false;
        }
        let path = PathBuf::from(trimmed);
        if !path.exists() {
            return false;
        }
        let kind = infer_attachment_kind(&path);
        let Ok(attachment) = validated_attachment(trimmed, kind) else {
            return false;
        };
        self.composer
            .draft
            .attach_file(attachment.as_path_buf(), attachment.kind);
        self.refresh_command_suggestions();
        true
    }

    pub(super) fn push_status(&mut self, title: impl Into<String>, items: Vec<RenderStatusItem>) {
        let line = RenderCellRecord::command_with_data(RenderCommandCell::new(
            LOCAL_COMMAND_STATUS_PART_ID,
            title,
            items,
        ));
        self.upsert_command_surface_line(line);
    }

    fn push_notice_status(&mut self, title: impl Into<String>, items: Vec<RenderStatusItem>) {
        let line = RenderCellRecord::status_with_data(RenderStatusCell {
            id: LOCAL_NOTICE_STATUS_PART_ID.to_owned(),
            title: title.into(),
            items,
        });
        self.upsert_command_surface_line(line);
    }

    fn upsert_command_surface_line(&mut self, line: RenderCellRecord) {
        if let Some(index) = self.cells.transcript_surfaces.command_status_cell()
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            existing.replace_with(line);
            return;
        }
        self.cells.transcript.push(line);
        self.cells
            .transcript_surfaces
            .set_command_status_cell(self.cells.transcript.len().saturating_sub(1));
    }

    pub(in crate::r#loop::agent_tui) fn push_history_item(
        &mut self,
        title: impl Into<String>,
        name: impl Into<String>,
        kind_label: impl Into<String>,
        status_label: Option<String>,
    ) {
        let title = title.into();
        let kind_label = kind_label.into();
        let key = format!("{title}:{kind_label}");
        let line = RenderCellRecord::history_with_data(AgentRenderHistoryCell {
            id: format!("history:local:{key}"),
            title,
            empty_text: "无记录".to_owned(),
            items: vec![AgentRenderHistoryItem::local(
                name,
                kind_label,
                status_label,
            )],
        });
        if let Some(index) = self.cells.transcript_surfaces.local_history_cell(&key)
            && let Some(existing) = self.cells.transcript.get_mut(index)
        {
            existing.replace_with(line);
            return;
        }
        self.cells.transcript.push(line);
        self.cells
            .transcript_surfaces
            .set_local_history_cell(key, self.cells.transcript.len().saturating_sub(1));
    }

    pub(in crate::r#loop::agent_tui) fn push_local_error(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        next_step: Option<String>,
    ) {
        self.cells
            .transcript
            .push(RenderCellRecord::error_with_data(local_error_cell(
                LOCAL_PUSH_ERROR_PART_ID,
                title,
                message,
                next_step,
            )));
    }

    pub(in crate::r#loop::agent_tui) fn push_runtime_error(&mut self, error: RuntimeError) {
        self.apply_runtime_error_output(runtime_error_projection::runtime_error_output(
            error, None,
        ));
        self.bridge.runtime_submission_enabled = false;
        self.header.hint = TUI_HINT_IDLE;
    }

    /// 展示一条后台 Execution 的完成通知。
    ///
    /// 通知只有真正写入 transcript 才算送达；调用方随后才会回执确认，避免静默 ack。
    pub(in crate::r#loop::agent_tui) fn push_completion_notification(&mut self, content: &str) {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return;
        }
        for (index, line) in normalize_agent_output(trimmed, "").iter().enumerate() {
            let text = if index == 0 {
                format!("{COMPLETION_NOTICE_PREFIX}{line}")
            } else {
                line.clone()
            };
            self.cells.transcript.push(RenderCellRecord::system(text));
        }
    }

    /// 覆盖式展示完成通知收件箱的最近一次失败，避免后台失败刷屏。
    pub(in crate::r#loop::agent_tui) fn push_completion_inbox_failure(&mut self, message: &str) {
        self.push_notice_status(
            format!("{COMPLETION_INBOX_FAILURE_TITLE}{message}"),
            Vec::new(),
        );
    }
}
