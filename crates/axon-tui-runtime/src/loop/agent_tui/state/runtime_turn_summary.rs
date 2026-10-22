//! Runtime turn summary cell 生命周期。

use std::time::Duration;

use axon_time::duration_ms;

use crate::{
    r#loop::agent_tui::{
        AgentTuiState, AgentTuiTokenUsage, runtime_bridge::GenerationPerformanceSnapshot,
    },
    surface::{cell::RenderCellRecord, cell_turn_summary_data::AgentRenderTurnSummaryCell},
};

const TURN_SUMMARY_ID_PREFIX: &str = "turn-summary";

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn reset_turn_work_activity(&mut self) {
        self.bridge.turn_work_activity = false;
    }

    pub(in crate::r#loop::agent_tui) fn record_turn_work_activity(&mut self) {
        self.bridge.turn_work_activity = true;
    }

    pub(in crate::r#loop::agent_tui) fn push_turn_summary_if_needed(
        &mut self,
        elapsed: Duration,
        usage: AgentTuiTokenUsage,
    ) {
        if !self.bridge.turn_work_activity {
            return;
        }
        self.bridge.turn_work_activity = false;
        self.push_turn_summary(Some(duration_ms(elapsed)), usage);
    }

    pub(in crate::r#loop::agent_tui) fn push_historical_turn_summary(
        &mut self,
        usage: AgentTuiTokenUsage,
    ) {
        self.push_turn_summary(None, usage);
    }

    fn push_turn_summary(&mut self, duration_ms: Option<u64>, usage: AgentTuiTokenUsage) {
        self.bridge.turn_work_activity = false;
        let performance = self.bridge.generation_performance.last;
        self.cells
            .transcript
            .push(RenderCellRecord::turn_summary_with_data(
                AgentRenderTurnSummaryCell::new(
                    self.next_turn_summary_id(),
                    duration_ms,
                    usage.has_tokens().then_some(usage),
                )
                .with_performance(
                    performance.and_then(GenerationPerformanceSnapshot::first_token_avg_ms),
                    performance.and_then(GenerationPerformanceSnapshot::generation_tps_x10),
                ),
            ));
    }

    fn next_turn_summary_id(&self) -> String {
        let turn = if self.header.session_summary.turns > 0 {
            self.header.session_summary.turns
        } else {
            u64::try_from(self.cells.transcript.len()).unwrap_or(u64::MAX)
        };
        format!("{TURN_SUMMARY_ID_PREFIX}:{turn}")
    }
}
