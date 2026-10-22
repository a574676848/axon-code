//! Agent TUI 帧输出稳定性签名。

use std::hash::{Hash, Hasher};

use super::AgentTuiState;

/// 单帧渲染输入签名，只用于内存内重复帧判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::r#loop::agent_tui) struct FrameRenderSignature(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TranscriptRenderStateSignature {
    transcript_revision: u64,
    transcript_signature: crate::surface::cell_store::TranscriptCellsSignature,
    active_runtime_revision: u64,
    tool_output_count: usize,
}

impl TranscriptRenderStateSignature {
    fn new(state: &AgentTuiState) -> Self {
        Self {
            transcript_revision: state.cells.transcript.transcript_revision(),
            transcript_signature: state.cells.transcript.transcript_signature(),
            active_runtime_revision: state.cells.active_runtime_cells.active_runtime_revision(),
            tool_output_count: state.cells.tool_outputs_by_part_id.len(),
        }
    }
}

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn transcript_render_signature(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        TranscriptRenderStateSignature::new(self).hash(&mut hasher);
        self.viewport.transcript_view_offset_rows.hash(&mut hasher);
        self.viewport.selected_runtime_part_id().hash(&mut hasher);
        self.viewport.frame_size.hash(&mut hasher);
        super::super::render::header_bar_height(self, self.viewport.frame_size.width)
            .hash(&mut hasher);
        self.composer_height().hash(&mut hasher);
        self.visible_text_selection.active_range().hash(&mut hasher);
        hasher.finish()
    }

    pub(in crate::r#loop::agent_tui) fn frame_render_signature(&self) -> FrameRenderSignature {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.hash_header_signature(&mut hasher);
        self.hash_viewport_signature(&mut hasher);
        self.hash_composer_signature(&mut hasher);
        self.hash_toast_signature(&mut hasher);
        self.hash_cell_signature(&mut hasher);
        self.hash_bridge_signature(&mut hasher);

        FrameRenderSignature(hasher.finish())
    }

    fn hash_header_signature(&self, hasher: &mut impl Hasher) {
        self.header.hint.hash(hasher);
        self.header.session_usage.total_tokens().hash(hasher);
        self.header.last_turn_usage.total_tokens().hash(hasher);
        self.header.live_turn_usage.total_tokens().hash(hasher);
        self.header.lsp_active_count.hash(hasher);
        self.header.replay_snapshot.hash(hasher);
    }

    fn hash_viewport_signature(&self, hasher: &mut impl Hasher) {
        self.viewport.transcript_view_offset_rows.hash(hasher);
        self.viewport.max_transcript_view_offset_rows.hash(hasher);
        self.viewport.transcript_view_page_rows.hash(hasher);
        self.viewport
            .transcript_view_scroll_state
            .as_str()
            .hash(hasher);
        self.viewport
            .cold_bottom_mount_phase()
            .as_str()
            .hash(hasher);
        let jump_affordance = self.viewport.transcript_view_jump_affordance();
        jump_affordance.available.hash(hasher);
        jump_affordance.distance_rows.hash(hasher);
        jump_affordance.threshold_rows.hash(hasher);
        self.viewport.selected_runtime_part_id().hash(hasher);
        self.visible_text_selection.active_range().hash(hasher);
        (
            self.viewport.frame_size.width,
            self.viewport.frame_size.height,
        )
            .hash(hasher);
    }

    fn hash_composer_signature(&self, hasher: &mut impl Hasher) {
        self.composer.draft.text().hash(hasher);
        self.composer.draft.cursor().hash(hasher);
        self.composer.command_suggestions.len().hash(hasher);
        self.composer.selected_command_suggestion.hash(hasher);
        self.hash_composer_popup_signature(hasher);
        self.composer.bottom_views.top_kind().hash(hasher);
        self.composer
            .bottom_views
            .top_lifecycle()
            .map(crate::app_shell::composer_dock::DockLifecycle::visible_height)
            .hash(hasher);
        self.composer.history_index.hash(hasher);
        self.composer
            .input_queue
            .queued_user_messages
            .len()
            .hash(hasher);
        self.composer.input_queue.pending_steers.len().hash(hasher);
        let subagent_activity = self.subagent_activity_snapshot(std::time::Instant::now());
        subagent_activity.rows.len().hash(hasher);
        for row in subagent_activity.rows {
            row.agent_id.hash(hasher);
            row.label.hash(hasher);
            row.status.hash(hasher);
            row.elapsed.as_secs().hash(hasher);
            row.usage.hash(hasher);
            row.model.hash(hasher);
            row.summary.hash(hasher);
        }
    }

    fn hash_composer_popup_signature(&self, hasher: &mut impl Hasher) {
        let Some(popup) = self.composer.composer_popup.as_ref() else {
            false.hash(hasher);
            return;
        };
        true.hash(hasher);
        popup.title().hash(hasher);
        popup.selected.hash(hasher);
        popup.items.len().hash(hasher);
        for item in &popup.items {
            item.value.hash(hasher);
            item.description.hash(hasher);
            item.insert_text.hash(hasher);
            item.mention_type.label().hash(hasher);
        }
    }

    fn hash_cell_signature(&self, hasher: &mut impl Hasher) {
        TranscriptRenderStateSignature::new(self).hash(hasher);
    }

    fn hash_toast_signature(&self, hasher: &mut impl Hasher) {
        self.toast.signature(std::time::Instant::now()).hash(hasher);
    }

    fn hash_bridge_signature(&self, hasher: &mut impl Hasher) {
        self.bridge.runtime_status.as_str().hash(hasher);
        self.bridge.spinner_frame.hash(hasher);
        self.bridge
            .live_status
            .display_elapsed(std::time::Instant::now())
            .map(|duration| duration.as_secs())
            .hash(hasher);
        self.bridge.subscription_phase.hash(hasher);
        self.bridge.assistant_streams.stream_count().hash(hasher);
        for (part_id, anchor) in self.bridge.assistant_streams.anchors() {
            part_id.hash(hasher);
            anchor.hash(hasher);
        }
        self.bridge.active_reasoning_parts.hash(hasher);
        self.bridge.live_lifecycle_line.is_some().hash(hasher);
        self.bridge.live_plan_line.is_some().hash(hasher);
        self.bridge.live_review_line.is_some().hash(hasher);
        self.bridge.live_hook_line.is_some().hash(hasher);
        self.bridge.interrupt_requested.hash(hasher);
        self.bridge.should_exit.hash(hasher);
        self.bridge.runtime_submission_enabled.hash(hasher);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        r#loop::agent_tui::state::runtime_flow_test_support::test_state,
        surface::cell::RenderCellRecord,
    };

    fn popup_skill_item(
        value: &str,
    ) -> crate::r#loop::agent_tui::composer_popup::ComposerPopupItem {
        use crate::r#loop::agent_tui::composer_popup::{
            ComposerPopupItem, ComposerPopupMentionType,
        };

        ComposerPopupItem {
            value: value.to_owned(),
            description: "skill".to_owned(),
            insert_text: format!("{value} "),
            target: format!("skill://{}", value.trim_start_matches('$')),
            mention_type: ComposerPopupMentionType::Skill,
            match_indices: None,
            score: 0,
        }
    }

    #[test]
    fn signature_changes_when_equal_length_draft_text_changes() {
        let mut state = test_state();
        state.composer.draft.replace_all("abc".to_owned());
        let first = state.frame_render_signature();

        state.composer.draft.replace_all("xyz".to_owned());

        assert_ne!(state.frame_render_signature(), first);
    }

    #[test]
    fn signature_stays_stable_without_render_state_changes() {
        let state = test_state();

        assert_eq!(
            state.frame_render_signature(),
            state.frame_render_signature()
        );
    }

    #[test]
    fn transcript_signature_ignores_composer_text_when_layout_is_unchanged() {
        let mut state = test_state();
        state.replace_draft("abc".to_owned());
        let first = state.transcript_render_signature();

        state.replace_draft("xyz".to_owned());

        assert_eq!(state.transcript_render_signature(), first);
    }

    #[test]
    fn signature_reuses_cached_transcript_hash_between_unchanged_frames() {
        let mut state = test_state();
        state.cells.transcript.clear();
        for index in 0..64 {
            state.cells.transcript.push(RenderCellRecord::agent_message(
                format!("assistant-part:cached:{index}"),
                format!("稳定回答 {index}"),
            ));
        }

        let _ = state.frame_render_signature();
        let rebuilds = state.cells.transcript.transcript_signature_rebuilds();
        let _ = state.frame_render_signature();

        assert_eq!(
            state.cells.transcript.transcript_signature_rebuilds(),
            rebuilds
        );
    }

    #[test]
    fn signature_changes_when_cold_bottom_mount_expands_to_full() {
        let mut state = test_state();
        state.cells.transcript.clear();
        for index in 0..12 {
            state
                .cells
                .transcript
                .push(RenderCellRecord::system(format!("运行行 {index}")));
            state.cells.active_runtime_cells.set_tool_cell(
                format!("call-{index}"),
                index,
                std::time::Instant::now(),
            );
        }

        state.prepare_cold_bottom_mount();
        let tail_only = state.frame_render_signature();
        state.complete_cold_bottom_mount_frame();

        assert_ne!(state.frame_render_signature(), tail_only);
    }

    #[test]
    fn signature_changes_when_selected_runtime_part_changes() {
        let mut state = test_state();
        state
            .viewport
            .select_runtime_part(0, "part:alpha".to_owned());
        let first = state.frame_render_signature();

        state
            .viewport
            .select_runtime_part(0, "part:beta".to_owned());

        assert_ne!(state.frame_render_signature(), first);
    }

    #[test]
    fn signature_changes_when_composer_popup_selection_changes() {
        use crate::r#loop::agent_tui::composer_popup::{ComposerPopup, ComposerPopupKind};

        let mut state = test_state();
        state.composer.composer_popup = Some(ComposerPopup::from_items(
            ComposerPopupKind::Skill,
            vec![popup_skill_item("$review"), popup_skill_item("$refactor")],
        ));
        let first = state.frame_render_signature();

        state.select_next_composer_popup_item();

        assert_ne!(state.frame_render_signature(), first);
    }

    #[test]
    fn transcript_signature_changes_when_visible_text_selection_changes() {
        let mut state = test_state();
        let first = state.transcript_render_signature();

        state.visible_text_selection.begin(0, 0);
        assert!(state.visible_text_selection.update(2, 4));

        assert_ne!(state.transcript_render_signature(), first);
    }

    #[test]
    fn signature_ignores_selected_runtime_render_index_when_part_identity_is_stable() {
        let mut state = test_state();
        state
            .viewport
            .select_runtime_part(0, "part:stable".to_owned());
        let first = state.frame_render_signature();

        state
            .viewport
            .select_runtime_part(2, "part:stable".to_owned());

        assert_eq!(state.frame_render_signature(), first);
    }

    #[test]
    fn signature_ignores_recreated_render_id_when_part_identity_and_content_are_stable() {
        let mut state = test_state();
        state.cells.transcript.clear();
        state.cells.transcript.push(RenderCellRecord::agent_message(
            "assistant-part:stable",
            "稳定回答",
        ));
        let first = state.frame_render_signature();

        state.cells.transcript.clear();
        state.cells.transcript.push(RenderCellRecord::agent_message(
            "assistant-part:stable",
            "稳定回答",
        ));

        assert_eq!(state.frame_render_signature(), first);
    }

    #[test]
    fn signature_tracks_cell_revision_instead_of_full_text_hash() {
        let mut state = test_state();
        state.cells.transcript.clear();
        state.cells.transcript.push(RenderCellRecord::agent_message(
            "assistant-part:revision",
            "aaaa",
        ));
        let first = state.frame_render_signature();

        state.cells.transcript[0].replace_with(RenderCellRecord::agent_message(
            "assistant-part:revision",
            "bbbb",
        ));

        assert_ne!(state.frame_render_signature(), first);
    }
}
