//! Agent TUI 可观测性状态接入。

use std::time::Duration;

use crate::surface::mailbox_telemetry::MailboxTelemetry;
use crate::surface::telemetry::TuiTelemetrySnapshot;

use super::AgentTuiState;
use crate::r#loop::agent_tui::frame_scheduler::FrameScheduleTelemetry;

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn attach_mailbox_telemetry(
        &mut self,
        telemetry: MailboxTelemetry,
    ) {
        self.bridge.telemetry.attach_mailbox_telemetry(telemetry);
    }

    pub(in crate::r#loop::agent_tui) fn telemetry_snapshot(&mut self) -> TuiTelemetrySnapshot {
        let cache = self.viewport.transcript_render_cache.take_metrics_delta();
        self.bridge.telemetry.record_cache(cache.hits, cache.misses);
        self.bridge.telemetry.record_resize_reflow(
            cache.resize_reflow.events,
            cache.resize_reflow.retained_entries,
            cache.resize_reflow.evicted_entries,
        );
        self.bridge
            .telemetry
            .record_render_profiler(cache.render_profiler);
        self.bridge.telemetry.snapshot()
    }

    pub(in crate::r#loop::agent_tui) fn record_frame_draw(&mut self, latency: Duration) {
        self.bridge.telemetry.record_frame_draw(latency);
    }

    pub(in crate::r#loop::agent_tui) fn record_stable_duplicate_frame(&mut self) {
        self.bridge.telemetry.record_stable_duplicate_frame();
    }
    pub(in crate::r#loop::agent_tui) fn record_streaming_delta_latency(
        &mut self,
        latency: Duration,
    ) {
        self.bridge
            .telemetry
            .record_streaming_delta_latency(latency);
    }
}

impl FrameScheduleTelemetry for AgentTuiState {
    fn record_frame_scheduled(&mut self) {
        self.bridge.telemetry.record_frame_scheduled();
    }

    fn record_frame_emitted(&mut self) {
        self.bridge.telemetry.record_frame_emitted();
    }

    fn record_frame_coalesced(&mut self, count: u64) {
        self.bridge.telemetry.record_frame_coalesced(count);
    }

    fn record_frame_disconnected(&mut self) {
        self.bridge.telemetry.record_frame_disconnected();
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Size;

    use crate::{
        r#loop::agent_tui::state::runtime_flow_test_support::test_state,
        surface::cell::RenderCellRecord,
    };

    #[test]
    fn resize_reflow_metrics_reach_telemetry_snapshot() {
        let mut state = test_state();
        let line = RenderCellRecord::agent_message(
            "assistant:test:telemetry-resize",
            "resize telemetry warm cache",
        );
        let _ = state
            .viewport
            .transcript_render_cache
            .render_cell_hyperlink_lines(&line, false, 80);
        let _ = state
            .viewport
            .transcript_render_cache
            .render_cell_hyperlink_lines(&line, false, 83);

        state.set_frame_size(Size::new(84, 24));
        let snapshot = state.telemetry_snapshot();

        assert_eq!(snapshot.resize_reflow_count, 1);
        assert_eq!(snapshot.resize_reflow_retained_entries, 2);
        assert_eq!(snapshot.resize_reflow_evicted_entries, 0);
    }
}
