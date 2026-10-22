//! Runtime assistant 正文首字与快照收口。

use std::time::Instant;

use axon_memory::project_memory_citation_text;

use crate::r#loop::agent_tui::state::AgentTuiState;
use crate::surface::cell::RenderCellData;

use super::runtime_flow_helpers::fallback_reply_lifecycle_status;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn apply_assistant_delta_batch(
        &mut self,
        part_id: String,
        deltas: Vec<String>,
    ) {
        let delta = deltas.concat();
        if delta.is_empty() {
            return;
        }
        self.record_visible_turn_activity();
        let now = Instant::now();
        self.settle_first_visible_assistant_content();
        self.push_live_assistant_delta(&part_id, &delta, now);
    }

    pub(in crate::r#loop::agent_tui) fn apply_assistant_message_snapshot(
        &mut self,
        part_id: String,
        content: String,
    ) {
        let content = project_memory_citation_text(&content);
        if content.trim().is_empty() {
            return;
        }
        self.record_visible_turn_activity();
        self.settle_first_visible_assistant_content();
        self.commit_assistant_message_snapshot(part_id, content);
    }

    fn settle_first_visible_assistant_content(&mut self) {
        if !self.has_live_assistant_stream() && !self.has_live_lifecycle_line() {
            self.apply_lifecycle_status(fallback_reply_lifecycle_status(
                &self.header.config_status,
                &self.header.overrides,
                self.models(),
            ));
        }
        self.clear_generation_lifecycle_status();
    }

    /// 结束“生成回复中”这一瞬时状态行：首个正文可见或 provider 流收口时都适用。
    pub(super) fn clear_generation_lifecycle_status(&mut self) {
        if self
            .bridge
            .live_lifecycle_line
            .as_ref()
            .is_some_and(|line| matches!(line.data, Some(RenderCellData::GenerationStatus(_))))
        {
            self.bridge.live_lifecycle_line = None;
        }
    }
}
