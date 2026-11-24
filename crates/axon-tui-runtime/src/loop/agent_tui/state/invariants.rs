//! 五段 sub-state 的 transaction 一致性审计。

use crate::r#loop::agent_tui::state::{AgentRuntimeStatus, AgentTuiState};
use crate::surface::cell::{ROLE_USER, RenderCellData, RenderCellKind, RenderCellMode};
use crate::surface::invariants::{
    TuiInvariantStateSnapshot, TuiInvariantViolation, TuiInvariantViolationDump, TuiStateSlice,
};
use crate::surface::render_policy::RenderPolicy;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn invariant_snapshot(&self) -> TuiInvariantStateSnapshot {
        if !cfg!(debug_assertions) {
            return TuiInvariantStateSnapshot::default();
        }
        TuiInvariantStateSnapshot {
            header: format!(
                "hint={};session={};usage_total={}",
                self.header.hint,
                self.header.overrides.session.as_deref().unwrap_or_default(),
                self.header.session_usage.total_tokens()
            ),
            viewport: format!(
                "dock={:?};selected={:?};transcript_view_offset={}/{};scroll={}",
                self.active_dock_kind(),
                self.viewport.selected_runtime_selection(),
                self.viewport.transcript_view_offset_rows,
                self.viewport.max_transcript_view_offset_rows,
                self.viewport.transcript_view_scroll_state.as_str()
            ),
            composer: format!(
                "draft_len={};submitted={};history_len={};queued={};pending_steers={};pending_start={}",
                self.composer.draft.text().chars().count(),
                self.composer
                    .submitted_prompt
                    .as_deref()
                    .unwrap_or_default(),
                self.composer.history.len(),
                self.composer.input_queue.queued_user_messages.len(),
                self.composer.input_queue.pending_steers.len(),
                self.composer.input_queue.user_turn_pending_start
            ),
            cells: format!(
                "transcript_len={};active_parts={:?};surfaces={:?}",
                self.cells.transcript.len(),
                self.cells.active_runtime_cells.active_runtime_parts(),
                self.cells.transcript_surfaces
            ),
            bridge: format!(
                "runtime={};assistant_anchors={:?};reasoning_parts={:?};assistant_streams={};interrupt={}",
                self.bridge.runtime_status.as_str(),
                self.bridge
                    .assistant_streams
                    .anchors()
                    .map(|(part_id, anchor)| (part_id, anchor.cell_index()))
                    .collect::<Vec<_>>(),
                self.bridge.active_reasoning_parts,
                self.bridge.assistant_streams.stream_count(),
                self.bridge.interrupt_requested
            ),
        }
    }

    pub(in crate::r#loop::agent_tui) fn collect_invariant_violations(
        &self,
    ) -> Vec<TuiInvariantViolation> {
        if !cfg!(debug_assertions) {
            return Vec::new();
        }
        let mut violations = Vec::new();
        self.check_working_turn_invariant(&mut violations);
        self.check_viewport_anchor_invariant(&mut violations);
        self.check_submit_projection_invariant(&mut violations);
        self.check_streaming_tail_invariant(&mut violations);
        self.check_input_queue_history_invariant(&mut violations);
        violations
    }

    pub(in crate::r#loop::agent_tui) fn debug_assert_transaction_invariants(
        &self,
        transaction: &'static str,
        before: TuiInvariantStateSnapshot,
    ) {
        if !cfg!(debug_assertions) {
            return;
        }
        let violations = self.collect_invariant_violations();
        if violations.is_empty() {
            return;
        }
        let mut dump = TuiInvariantViolationDump::new(
            transaction,
            before,
            self.invariant_snapshot(),
            violations,
        );
        if let Ok(path) = self.write_state_dump_for_debug(transaction) {
            dump = dump.with_state_dump_path(path.display().to_string());
        }
        debug_assert!(false, "{dump}");
    }

    fn check_working_turn_invariant(&self, violations: &mut Vec<TuiInvariantViolation>) {
        let working = self.bridge.runtime_status == AgentRuntimeStatus::Working;
        if working && self.composer.submitted_prompt.is_none() {
            violations.push(TuiInvariantViolation::new(
                "working_without_submitted_prompt",
                "runtime_status=Working 时必须存在 submitted_prompt 作为 active turn 锚点",
                vec![TuiStateSlice::Bridge, TuiStateSlice::Composer],
            ));
        }
        if !working && self.composer.input_queue.user_turn_pending_start {
            violations.push(TuiInvariantViolation::new(
                "idle_with_pending_turn_start",
                "runtime_status=Ready 后 input_queue.user_turn_pending_start 必须清空",
                vec![TuiStateSlice::Bridge, TuiStateSlice::Composer],
            ));
        }
    }

    fn check_viewport_anchor_invariant(&self, violations: &mut Vec<TuiInvariantViolation>) {
        self.check_transcript_viewport_bounds(violations);
        self.check_runtime_selection_anchor(violations);
        self.check_active_assistant_anchors(violations);
        self.check_active_reasoning_anchors(violations);
        self.check_active_runtime_part_anchors(violations);
    }

    fn check_transcript_viewport_bounds(&self, violations: &mut Vec<TuiInvariantViolation>) {
        if self.viewport.transcript_view_offset_rows > self.viewport.max_transcript_view_offset_rows
        {
            violations.push(TuiInvariantViolation::new(
                "transcript_view_offset_out_of_bounds",
                format!(
                    "transcript view offset {} 超出 max {}",
                    self.viewport.transcript_view_offset_rows,
                    self.viewport.max_transcript_view_offset_rows
                ),
                vec![TuiStateSlice::Viewport],
            ));
        }
        if self.viewport.transcript_view_offset_rows > 0
            && self.viewport.transcript_view_scroll_state.follows_tail()
        {
            violations.push(TuiInvariantViolation::new(
                "transcript_view_following_tail_with_offset",
                format!(
                    "scroll state {} 不能同时保持 offset {}",
                    self.viewport.transcript_view_scroll_state.as_str(),
                    self.viewport.transcript_view_offset_rows
                ),
                vec![TuiStateSlice::Viewport],
            ));
        }
        let jump_affordance = self.viewport.transcript_view_jump_affordance();
        if jump_affordance.available
            && (self.viewport.transcript_view_scroll_state
                != crate::r#loop::agent_tui::state::viewport_state::ViewportScrollState::JumpAvailable
                || jump_affordance.distance_rows <= jump_affordance.threshold_rows)
        {
            violations.push(TuiInvariantViolation::new(
                "transcript_view_jump_affordance_invalid",
                format!(
                    "jump affordance 状态不一致：scroll={} distance={} threshold={}",
                    self.viewport.transcript_view_scroll_state.as_str(),
                    jump_affordance.distance_rows,
                    jump_affordance.threshold_rows
                ),
                vec![TuiStateSlice::Viewport],
            ));
        }
    }

    fn check_runtime_selection_anchor(&self, violations: &mut Vec<TuiInvariantViolation>) {
        if let Some(selection) = self.viewport.selected_runtime_selection() {
            let index = selection.cell_index();
            let valid = self
                .cells
                .transcript
                .get(index)
                .is_some_and(|line| RenderPolicy::decide(line).selectable);
            if !valid {
                violations.push(TuiInvariantViolation::new(
                    "selected_runtime_render_index_missing",
                    format!("selected_runtime_render_index {index} 未指向存在的 runtime cell"),
                    vec![TuiStateSlice::Viewport, TuiStateSlice::Cells],
                ));
            }
        }
        if let Some(selection) = self.viewport.selected_runtime_selection() {
            let part_id = selection.part_id();
            let valid = self
                .cells
                .transcript
                .iter()
                .any(|line| line.semantic_part_id().as_deref() == Some(part_id));
            if !valid {
                violations.push(TuiInvariantViolation::new(
                    "selected_runtime_part_missing",
                    format!("selected_runtime_part_id {part_id} 未指向存在的 runtime part"),
                    vec![TuiStateSlice::Viewport, TuiStateSlice::Cells],
                ));
            }
        }
        if let Some(selection) = self.viewport.selected_runtime_selection()
            && self
                .cells
                .transcript
                .get(selection.cell_index())
                .is_some_and(|line| line.semantic_part_id().as_deref() != Some(selection.part_id()))
        {
            violations.push(TuiInvariantViolation::new(
                "selected_runtime_identity_mismatch",
                format!(
                    "selected_runtime_render_index {} 与 part_id {} 不一致",
                    selection.cell_index(),
                    selection.part_id()
                ),
                vec![TuiStateSlice::Viewport, TuiStateSlice::Cells],
            ));
        }
    }

    fn check_active_assistant_anchors(&self, violations: &mut Vec<TuiInvariantViolation>) {
        for (part_id, anchor) in self.bridge.assistant_streams.anchors() {
            let index = anchor.cell_index();
            let valid = anchor.part_id() == part_id
                && self
                    .cells
                    .transcript
                    .get(index)
                    .is_some_and(|line| match &line.data {
                        Some(RenderCellData::Message(cell)) => cell.id == part_id,
                        _ => false,
                    });
            if !valid {
                violations.push(TuiInvariantViolation::new(
                    "active_assistant_cell_missing",
                    format!(
                        "active_assistant_anchor {part_id} -> {index} 未指向对应 assistant message cell"
                    ),
                    vec![TuiStateSlice::Bridge, TuiStateSlice::Cells],
                ));
            }
        }
    }

    fn check_active_reasoning_anchors(&self, violations: &mut Vec<TuiInvariantViolation>) {
        for (part_id, state) in &self.bridge.active_reasoning_parts {
            let Some(anchor) = state.anchor() else {
                continue;
            };
            let index = anchor.cell_index();
            let anchor_part_id = anchor.part_id();
            let valid = part_id == anchor_part_id
                && self
                    .cells
                    .transcript
                    .get(index)
                    .is_some_and(|line| match &line.data {
                        Some(RenderCellData::Reasoning(cell)) => cell.id == *part_id,
                        _ => false,
                    });
            if !valid {
                violations.push(TuiInvariantViolation::new(
                    "active_reasoning_cell_missing",
                    format!("active reasoning part {part_id} -> {index} 未指向对应 reasoning cell"),
                    vec![TuiStateSlice::Bridge, TuiStateSlice::Cells],
                ));
            }
        }
    }

    fn check_active_runtime_part_anchors(&self, violations: &mut Vec<TuiInvariantViolation>) {
        for part in self.cells.active_runtime_cells.active_runtime_parts() {
            let valid = self.cells.transcript.get(part.index).is_some_and(|line| {
                line.semantic_part_id().as_deref() == Some(part.part_id.as_str())
            });
            if !valid {
                violations.push(TuiInvariantViolation::new(
                    "active_runtime_part_missing",
                    format!(
                        "active runtime part {} -> {} 未指向对应 transcript cell",
                        part.part_id, part.index
                    ),
                    vec![TuiStateSlice::Cells],
                ));
            }
        }
    }

    fn check_submit_projection_invariant(&self, violations: &mut Vec<TuiInvariantViolation>) {
        let Some(prompt) = self.composer.submitted_prompt.as_deref() else {
            return;
        };
        if self
            .header
            .replay_snapshot
            .is_some_and(crate::surface::replay_snapshot::ReplaySnapshotHeaderState::is_loading)
        {
            return;
        }
        if !self
            .cells
            .transcript
            .iter()
            .any(|line| line.role == ROLE_USER && line.text.trim() == prompt.trim())
        {
            violations.push(TuiInvariantViolation::new(
                "submitted_prompt_without_user_cell",
                "submitted_prompt 必须投影为 transcript user cell",
                vec![TuiStateSlice::Composer, TuiStateSlice::Cells],
            ));
        }
        if self.composer.draft.text().trim() == prompt.trim() && !prompt.trim().is_empty() {
            violations.push(TuiInvariantViolation::new(
                "submitted_prompt_left_in_draft",
                "提交后的原始 prompt 不应继续滞留在 draft 中",
                vec![TuiStateSlice::Composer],
            ));
        }
    }

    fn check_streaming_tail_invariant(&self, violations: &mut Vec<TuiInvariantViolation>) {
        for part_id in self.bridge.assistant_streams.stream_part_ids() {
            if self.bridge.assistant_streams.anchor(part_id).is_none()
                && self.cells.transcript.iter().any(|line| match &line.data {
                    Some(RenderCellData::Message(cell)) => cell.id == part_id,
                    _ => false,
                })
            {
                violations.push(TuiInvariantViolation::new(
                    "assistant_stream_without_active_tail_anchor",
                    format!(
                        "assistant streaming part {part_id} 已落 transcript 时必须保留 active anchor"
                    ),
                    vec![TuiStateSlice::Bridge, TuiStateSlice::Cells],
                ));
            }
        }
        if let Some(line) = &self.bridge.live_plan_line {
            assert_live_kind(
                line.mode,
                RenderCellKind::PLAN,
                "live_plan_line",
                violations,
            );
        }
        if let Some(line) = &self.bridge.live_review_line {
            assert_live_kind(
                line.mode,
                RenderCellKind::REVIEW,
                "live_review_line",
                violations,
            );
        }
        if let Some(line) = &self.bridge.live_hook_line {
            assert_live_kind(
                line.mode,
                RenderCellKind::HOOK,
                "live_hook_line",
                violations,
            );
        }
        if let Some(line) = &self.bridge.live_lifecycle_line {
            let valid = matches!(
                line.mode,
                RenderCellMode::Cell(RenderCellKind::GENERATION_STATUS | RenderCellKind::STATUS)
            );
            if !valid {
                violations.push(TuiInvariantViolation::new(
                    "live_lifecycle_kind_mismatch",
                    format!("live_lifecycle_line 不能是 {:?}", line.mode),
                    vec![TuiStateSlice::Bridge],
                ));
            }
        }
    }

    fn check_input_queue_history_invariant(&self, violations: &mut Vec<TuiInvariantViolation>) {
        let queue = &self.composer.input_queue;
        if queue.queued_user_messages.len() != queue.queued_user_message_history_records.len() {
            violations.push(TuiInvariantViolation::new(
                "queued_user_message_history_mismatch",
                "queued user messages 与 history records 数量必须一致",
                vec![TuiStateSlice::Composer],
            ));
        }
    }
}

fn assert_live_kind(
    mode: RenderCellMode,
    expected: RenderCellKind,
    field: &'static str,
    violations: &mut Vec<TuiInvariantViolation>,
) {
    if mode == RenderCellMode::Cell(expected) {
        return;
    }
    violations.push(TuiInvariantViolation::new(
        "live_tail_kind_mismatch",
        format!("{field} 不能是 {mode:?}，期望 {expected:?}"),
        vec![TuiStateSlice::Bridge],
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::agent_tui::runtime_bridge::{RuntimePartAnchor, RuntimePartState};
    use crate::r#loop::agent_tui::state::runtime_flow_test_support::{test_state, test_submission};
    use crate::surface::cell::{RenderCellRecord, RenderStatusCell};

    #[test]
    fn invariant_reports_working_without_active_turn_anchor() {
        let mut state = test_state();
        state.bridge.runtime_status = AgentRuntimeStatus::Working;

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "working_without_submitted_prompt");
    }

    #[test]
    fn invariant_reports_invalid_viewport_and_active_cell_anchors() {
        let mut state = test_state();
        state
            .viewport
            .select_runtime_part(99, "part:missing".to_owned());
        state.bridge.assistant_streams.set_anchor(
            "assistant:missing".to_owned(),
            RuntimePartAnchor::new("assistant:missing".to_owned(), 42),
        );
        state.bridge.active_reasoning_parts.insert(
            "reasoning:missing".to_owned(),
            RuntimePartState::Anchored(RuntimePartAnchor::new("reasoning:missing".to_owned(), 43)),
        );

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "selected_runtime_render_index_missing");
        assert_has_violation(&violations, "active_assistant_cell_missing");
        assert_has_violation(&violations, "active_reasoning_cell_missing");
    }

    #[test]
    fn invariant_reports_selected_runtime_identity_mismatch() {
        let mut state = test_state();
        state.cells.transcript.clear();
        state
            .cells
            .transcript
            .push(RenderCellRecord::agent_message("part:actual", "内容"));
        state
            .viewport
            .select_runtime_part(0, "part:stale".to_owned());

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "selected_runtime_identity_mismatch");
    }

    #[test]
    fn invariant_reports_submitted_prompt_without_user_cell() {
        let mut state = test_state();
        state.composer.submitted_prompt = Some("未投影".to_owned());
        state.composer.draft.replace_all("未投影".to_owned());

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "submitted_prompt_without_user_cell");
        assert_has_violation(&violations, "submitted_prompt_left_in_draft");
    }

    #[test]
    fn invariant_reports_streaming_tail_without_active_anchor() {
        let mut state = test_state();
        state.bridge.assistant_streams.push_delta(
            "assistant:test",
            "流式内容",
            80,
            std::path::Path::new(&state.header.cwd),
            std::time::Instant::now(),
        );
        state.cells.transcript.push(RenderCellRecord::agent_message(
            "assistant:test",
            "流式内容",
        ));

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "assistant_stream_without_active_tail_anchor");
    }

    #[test]
    fn invariant_reports_live_tail_kind_mismatch() {
        let mut state = test_state();
        state.bridge.live_plan_line = Some(RenderCellRecord::status_with_data(
            RenderStatusCell::from_tui_status(
                &crate::r#loop::agent_tui::run_output::AgentTuiStatusLine::new(
                    "错误类型",
                    vec![
                        crate::r#loop::agent_tui::run_output::AgentTuiStatusItem::new(
                            "kind", "summary", None, None, None, None,
                        ),
                    ],
                ),
            ),
        ));

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "live_tail_kind_mismatch");
    }

    #[test]
    fn invariant_reports_input_queue_history_mismatch() {
        let mut state = test_state();
        state.composer.input_queue.queued_user_messages.push_back(
            super::super::queued_input::QueuedUserMessage {
                user_message: super::super::queued_input::UserMessage {
                    submission: test_submission("后续输入"),
                },
            },
        );

        let violations = state.collect_invariant_violations();

        assert_has_violation(&violations, "queued_user_message_history_mismatch");
    }

    #[test]
    fn violation_dump_includes_transaction_and_sub_state_diff() {
        let mut state = test_state();
        let before = state.invariant_snapshot();
        state.bridge.runtime_status = AgentRuntimeStatus::Working;
        let violations = state.collect_invariant_violations();
        let dump = TuiInvariantViolationDump::new(
            "runtime_event:AssistantDelta",
            before,
            state.invariant_snapshot(),
            violations,
        )
        .to_string();

        assert!(dump.contains("runtime_event:AssistantDelta"));
        assert!(dump.contains("working_without_submitted_prompt"));
        assert!(dump.contains("before:"));
        assert!(dump.contains("after:"));
        assert!(dump.contains("bridge"));
        assert!(dump.contains("composer"));
    }

    #[test]
    fn violation_dump_includes_state_dump_path_when_available() {
        let dump = TuiInvariantViolationDump::new(
            "runtime_event:AssistantDelta",
            TuiInvariantStateSnapshot::default(),
            TuiInvariantStateSnapshot::default(),
            vec![TuiInvariantViolation::new(
                "working_without_submitted_prompt",
                "runtime_status=Working 时必须存在 submitted_prompt",
                vec![TuiStateSlice::Bridge],
            )],
        )
        .with_state_dump_path(r"C:\Temp\axon-tui-state-dumps\tui-state.json")
        .to_string();

        assert!(dump.contains("state dump:"));
        assert!(dump.contains("tui-state.json"));
    }

    #[test]
    fn valid_runtime_submit_satisfies_invariants() {
        let mut state = test_state();
        let submission = state.submit_plain("真实提交".to_owned());
        state.start_runtime_submit(&submission);

        assert!(state.collect_invariant_violations().is_empty());
    }

    fn assert_has_violation(violations: &[TuiInvariantViolation], code: &str) {
        assert!(
            violations.iter().any(|violation| violation.code == code),
            "missing violation {code}: {violations:#?}"
        );
    }
}
