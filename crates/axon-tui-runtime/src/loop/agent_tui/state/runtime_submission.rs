//! Agent TUI runtime 提交准备与启动。

use std::time::Instant;

use crate::r#loop::agent_tui::{
    TUI_HINT_WORKING, history_store,
    input::{AgentTuiInputElement, AgentTuiInputElementKind, AgentTuiInputResult},
    runtime_identity::effective_model_identity,
    state::{
        AgentRuntimeStatus, AgentTuiState, AgentTuiSubmit, command_handlers::AgentTuiCommandOutcome,
    },
};
use crate::surface::animation::transcript_motion_tokens;
use crate::surface::cell::RenderCellRecord;

use super::runtime_flow_helpers::fallback_reply_lifecycle_status;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn begin_submit(&mut self) -> Option<AgentTuiSubmit> {
        let before = self.invariant_snapshot();
        let submission = self.take_submit_from_draft()?;
        if !self.runtime_submission_enabled() {
            self.restore_submission_to_draft(submission.text(), &submission);
            self.push_local_error(
                "运行时不可用",
                "RuntimeAdapter 契约校验失败，已阻断本次提交。",
                Some("升级 Axon runtime adapter 后重试。".to_owned()),
            );
            self.debug_assert_transaction_invariants("begin_submit:runtime_disabled", before);
            return None;
        }
        self.start_runtime_submit(&submission);
        self.bridge.pending_runtime_submit = Some(submission.clone());
        self.debug_assert_transaction_invariants("begin_submit", before);
        Some(submission)
    }

    pub(in crate::r#loop::agent_tui) fn take_pending_runtime_submit(
        &mut self,
    ) -> Option<AgentTuiSubmit> {
        self.bridge.pending_runtime_submit.take()
    }

    /// 把未被消费的提交放回待提交位：调用方拒绝处理时必须原样交回，不能吞掉。
    pub(in crate::r#loop::agent_tui) fn restore_pending_runtime_submit(
        &mut self,
        submission: AgentTuiSubmit,
    ) {
        self.bridge.pending_runtime_submit = Some(submission);
    }

    /// 作答已送达统一答案入口：收起提问面板，等 settled 事件刷新单元状态。
    pub(in crate::r#loop::agent_tui) fn settle_question_answer_submission(&mut self) {
        self.close_question_view();
    }

    /// 作答未送达：保留面板与已填内容，并给出可执行的失败说明。
    pub(in crate::r#loop::agent_tui) fn push_question_answer_rejected_notice(
        &mut self,
        error: Option<crate::r#loop::agent_tui::RuntimeError>,
    ) {
        let detail = error.map_or_else(
            || "作答未送达统一答案入口。".to_owned(),
            |error| format!("作答未送达统一答案入口：{}", error.message()),
        );
        self.push_local_error(
            "问答作答未送达",
            detail,
            Some("面板已保留本次作答，可直接重试；如需放弃请按 Esc 收起。".to_owned()),
        );
    }

    pub(in crate::r#loop::agent_tui) fn take_submit_from_draft(
        &mut self,
    ) -> Option<AgentTuiSubmit> {
        let draft = self.composer.draft.text().trim().to_owned();
        if draft.is_empty() {
            return None;
        }
        if self.draft_blocks_unsupported_image(&draft) {
            return None;
        }
        match self.handle_command(&draft) {
            AgentTuiCommandOutcome::NotCommand => {}
            AgentTuiCommandOutcome::Handled => return None,
        }
        Some(self.submit_plain(draft))
    }

    pub(in crate::r#loop::agent_tui) fn submit_plain(&mut self, draft: String) -> AgentTuiSubmit {
        let (text, elements) = self.prepare_input_payload(&draft);
        self.submit_input(AgentTuiInputResult::Submit { text, elements }, true)
    }

    pub(in crate::r#loop::agent_tui) fn submit_queued(
        &mut self,
        draft: String,
        action: crate::r#loop::agent_tui::AgentTuiQueuedAction,
        record_history: bool,
    ) -> AgentTuiSubmit {
        let (text, elements) = self.prepare_input_payload(&draft);
        self.submit_input(
            AgentTuiInputResult::Queued {
                text,
                elements,
                action,
            },
            record_history,
        )
    }

    pub(in crate::r#loop::agent_tui) fn start_runtime_submit(
        &mut self,
        submission: &AgentTuiSubmit,
    ) {
        let before = self.invariant_snapshot();
        let now = Instant::now();
        self.bridge.telemetry.start_first_token_submit();
        self.flush_live_assistant_output();
        self.remove_startup_transcript_lines();
        self.cells
            .transcript
            .push(RenderCellRecord::user(submission.text().to_owned()));
        self.record_cancel_edit_candidate(submission);
        self.composer.submitted_prompt = Some(submission.text().to_owned());
        self.header.hint = TUI_HINT_WORKING;
        self.bridge.runtime_status = AgentRuntimeStatus::Working;
        self.bridge.reset_turn_ledger();
        self.advance_terminal_title_lifecycle();
        self.composer.input_queue.user_turn_pending_start = true;
        self.bridge.interrupt_requested = false;
        self.bridge.provider_activity.clear();
        // 提交先投影本地连接态；真实 ProviderStatus 会原位刷新并推进到连接成功。
        let identity = effective_model_identity(
            &self.header.config_status,
            &self.header.overrides,
            self.models(),
        );
        self.bridge
            .provider_activity
            .begin_connecting(&identity.provider, &identity.model, now);
        self.bridge.subagent_activity.resume();
        self.clear_active_transcript_lines();
        self.cells.active_runtime_cells.clear();
        self.reset_turn_work_activity();
        self.reset_live_turn_usage();
        self.bridge.generation_performance.turn_terminal = false;
        self.bridge.generation_performance.last_observation_at_ms = 0;
        self.bridge.live_status.reset_for_submit(now);
        self.apply_lifecycle_status(fallback_reply_lifecycle_status(
            &self.header.config_status,
            &self.header.overrides,
            self.models(),
        ));
        self.bridge.spinner_frame = 0;
        self.bridge.next_spinner_tick = now + transcript_motion_tokens().spinner;
        self.debug_assert_transaction_invariants("start_runtime_submit", before);
    }

    pub(in crate::r#loop::agent_tui) fn record_input_history(&mut self, text: &str) {
        if self
            .composer
            .history
            .last()
            .is_some_and(|item| item == text)
        {
            return;
        }
        self.composer.history.push(text.to_owned());
        if let Err(error) = history_store::persist_draft_history(
            &self.composer.history_path,
            &self.composer.history,
        ) {
            self.push_local_error("历史保存失败", error.to_string(), None);
        }
    }

    fn prepare_input_payload(&self, draft: &str) -> (String, Vec<AgentTuiInputElement>) {
        let elements = self.composer.draft.input_elements(draft);
        (self.composer.draft.expand_pending_pastes(draft), elements)
    }

    fn submit_input(&mut self, input: AgentTuiInputResult, record_history: bool) -> AgentTuiSubmit {
        let text = input.text().to_owned();
        if record_history {
            self.record_input_history(&text);
        }
        self.composer.history_index = None;
        self.clear_draft();
        AgentTuiSubmit {
            input,
            overrides: self.header.overrides.clone(),
            cwd: self.header.cwd.clone().into(),
            question_answer: None,
        }
    }

    /// 构造待决问答的作答提交。
    ///
    /// 作答正文由统一编码器在服务端生成，TUI 不自造正文，因此这里的输入文本
    /// 保持为空：它只承载车道身份与作答条目，不会进入本地转录或输入历史。
    pub(in crate::r#loop::agent_tui) fn question_answer_submission(
        &mut self,
        answer: crate::r#loop::agent_tui::AgentTuiQuestionAnswer,
    ) -> AgentTuiSubmit {
        AgentTuiSubmit {
            input: AgentTuiInputResult::Submit {
                text: String::new(),
                elements: Vec::new(),
            },
            overrides: self.header.overrides.clone(),
            cwd: self.header.cwd.clone().into(),
            question_answer: Some(answer),
        }
    }

    /// 当前模型是否声明支持视觉输入。
    ///
    /// 模型项缺失（配置刚变更、catalog 尚未刷新等）时返回 `None`：无法判定能力
    /// 就不阻断提交，交由运行时与 Provider 侧的既有降级路径处理。
    fn current_model_supports_vision(&self) -> Option<bool> {
        self.current_model_catalog_item()
            .map(|item| item.supports_vision)
    }

    /// 草稿校验：含图片附件而当前模型不支持视觉时阻断提交。
    ///
    /// 图片附件只以占位元素登记，提交前若不放行就会静默丢失视觉输入，
    /// 因此这里在草稿进入提交与排队两条入口之前先失败并给出可执行提示。
    pub(in crate::r#loop::agent_tui) fn draft_blocks_unsupported_image(
        &mut self,
        draft: &str,
    ) -> bool {
        let has_image = self
            .composer
            .draft
            .input_elements(draft)
            .iter()
            .any(|element| matches!(element.kind, AgentTuiInputElementKind::Image { .. }));
        if !has_image || self.current_model_supports_vision() != Some(false) {
            return false;
        }
        self.push_local_error(
            "当前模型不支持图片输入",
            "请切换到支持视觉的模型后重新提交；图片仍保留在草稿中。",
            Some("Alt+V 粘贴的图片只会以占位符进入提示词，模型无法看到图像内容。".to_owned()),
        );
        true
    }
}
