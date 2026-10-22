//! Header 状态事实源。

use crate::AgentTuiSessionSummary;
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiConfigStatus, AgentTuiRunOverrides, AgentTuiSettingsChange,
    AgentTuiTokenUsage, TUI_HINT_IDLE,
};
use crate::surface::replay_snapshot::ReplaySnapshotHeaderState;

/// Header 与全局会话摘要状态。
#[derive(Debug, Clone)]
pub(crate) struct HeaderState {
    pub(crate) cwd: String,
    pub(crate) workspace_instruction_paths: Vec<String>,
    pub(crate) config_status: AgentTuiConfigStatus,
    pub(crate) overrides: AgentTuiRunOverrides,
    pub(crate) catalog: AgentTuiCatalog,
    pub(crate) hint: &'static str,
    pub(crate) session_usage: AgentTuiTokenUsage,
    pub(crate) last_turn_usage: AgentTuiTokenUsage,
    /// 本轮读数：已收口流的累计加上在跑流的快照，界面只读这一个。
    pub(crate) live_turn_usage: AgentTuiTokenUsage,
    /// 本轮已收口 provider 流的用量累计。
    pub(crate) live_turn_usage_folded: AgentTuiTokenUsage,
    pub(crate) session_summary: AgentTuiSessionSummary,
    pub(crate) replay_snapshot: Option<ReplaySnapshotHeaderState>,
    pub(crate) settings_changes: Vec<AgentTuiSettingsChange>,
    pub(crate) lsp_active_count: usize,
    pub(crate) gateway_active_turn_count: usize,
}

impl HeaderState {
    pub(crate) fn new(request: &crate::r#loop::agent_tui::AgentTuiLaunchRequest) -> Self {
        Self {
            cwd: request.cwd.to_string_lossy().to_string(),
            workspace_instruction_paths: request.workspace_instruction_paths.clone(),
            config_status: request.config_status.clone(),
            overrides: request.overrides.clone(),
            catalog: request.catalog.clone(),
            hint: TUI_HINT_IDLE,
            session_usage: AgentTuiTokenUsage::default(),
            last_turn_usage: AgentTuiTokenUsage::default(),
            live_turn_usage: AgentTuiTokenUsage::default(),
            live_turn_usage_folded: AgentTuiTokenUsage::default(),
            session_summary: AgentTuiSessionSummary::default(),
            replay_snapshot: None,
            settings_changes: Vec::new(),
            lsp_active_count: 0,
            gateway_active_turn_count: 0,
        }
    }

    pub(crate) fn clear_handoff_state(&mut self) {
        self.hint = TUI_HINT_IDLE;
        self.live_turn_usage = AgentTuiTokenUsage::default();
        self.live_turn_usage_folded = AgentTuiTokenUsage::default();
        self.replay_snapshot = None;
        self.settings_changes.clear();
        self.lsp_active_count = 0;
        self.gateway_active_turn_count = 0;
    }
}
