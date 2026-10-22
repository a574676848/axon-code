//! Runtime token usage 生命周期。
//!
//! 用量帧按「provider 流」记账：一条流的用量帧是**该条流的累计快照**，且共享 producer
//! 会在流收口前把同一份快照再投一次。流内按覆盖处理、收口时并入累计，只有跨流才相加，
//! 否则整轮读数要么按重复帧翻倍，要么只剩最后一条流。

use crate::r#loop::agent_tui::{AgentTuiState, AgentTuiTokenUsage};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn reset_live_turn_usage(&mut self) {
        self.header.live_turn_usage = AgentTuiTokenUsage::default();
        self.header.live_turn_usage_folded = AgentTuiTokenUsage::default();
    }

    pub(in crate::r#loop::agent_tui) fn record_live_turn_usage(
        &mut self,
        usage: AgentTuiTokenUsage,
    ) {
        if !usage.has_tokens() {
            return;
        }
        self.header.live_turn_usage = self.header.live_turn_usage_folded.fold(usage);
        self.header.last_turn_usage = self.header.live_turn_usage;
    }

    /// 收口一条 provider 流：把在跑流的快照并入本轮累计。
    ///
    /// 此刻 `live_turn_usage` 已经等于「累计 + 该流快照」，因此直接把它当成新的累计，
    /// 下一条流的快照就会累加到它之上，而不是把它替换掉。
    pub(in crate::r#loop::agent_tui) fn fold_live_turn_usage(&mut self) {
        if !self.header.live_turn_usage.has_tokens() {
            return;
        }
        self.header.live_turn_usage_folded = self.header.live_turn_usage;
    }

    pub(in crate::r#loop::agent_tui) fn settle_turn_usage(
        &mut self,
        final_usage: AgentTuiTokenUsage,
    ) -> AgentTuiTokenUsage {
        let usage = if final_usage.has_tokens() {
            final_usage
        } else {
            self.header.live_turn_usage
        };
        if usage.has_tokens() {
            self.header.last_turn_usage = usage;
            self.header.session_usage.add_assign(usage);
        }
        self.reset_live_turn_usage();
        usage
    }
}
