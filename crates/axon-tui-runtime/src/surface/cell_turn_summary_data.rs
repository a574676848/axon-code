//! Turn summary cell 数据模型。

use crate::AgentTuiTokenUsage;

/// Codex 风格的 turn 收尾分割 cell。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRenderTurnSummaryCell {
    pub(crate) id: String,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) total_tokens: Option<u64>,
    pub(crate) usage: Option<AgentTuiTokenUsage>,
    pub(crate) first_token_avg_ms: Option<u64>,
    pub(crate) generation_tps_x10: Option<u64>,
}

impl AgentRenderTurnSummaryCell {
    pub(crate) fn new(
        id: impl Into<String>,
        duration_ms: Option<u64>,
        usage: Option<AgentTuiTokenUsage>,
    ) -> Self {
        Self {
            id: id.into(),
            duration_ms,
            total_tokens: usage.map(AgentTuiTokenUsage::total_tokens),
            usage,
            first_token_avg_ms: None,
            generation_tps_x10: None,
        }
    }

    pub(crate) const fn with_performance(
        mut self,
        first_token_avg_ms: Option<u64>,
        generation_tps_x10: Option<u64>,
    ) -> Self {
        self.first_token_avg_ms = first_token_avg_ms;
        self.generation_tps_x10 = generation_tps_x10;
        self
    }

    pub(crate) const fn with_total_tokens(mut self, total_tokens: Option<u64>) -> Self {
        if self.total_tokens.is_none() {
            self.total_tokens = total_tokens;
        }
        self
    }
}
