//! 子 Agent 活动区渲染模型。

use std::time::Duration;

pub(crate) const SUBAGENT_ACTIVITY_KIND: &str = "subagent_activity";
pub(crate) const SUBAGENT_ACTIVITY_MAX_VISIBLE_ROWS: usize = 4;
pub(crate) const SUBAGENT_ACTIVITY_MIN_HEIGHT: u16 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SubagentActivityStatus {
    Starting,
    Running,
    Waiting,
    Completed,
    Interrupted,
    Failed,
}

impl SubagentActivityStatus {
    pub(crate) const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Interrupted | Self::Failed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SubagentActivityRow {
    pub(crate) agent_id: String,
    pub(crate) label: String,
    pub(crate) status: SubagentActivityStatus,
    pub(crate) activity: String,
    pub(crate) elapsed: Duration,
    pub(crate) usage: Option<u64>,
    pub(crate) cache_hit_rate_basis_points: Option<u16>,
    pub(crate) model: Option<String>,
    pub(crate) summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct SubagentActivitySnapshot {
    pub(crate) rows: Vec<SubagentActivityRow>,
    pub(crate) max_visible_rows: usize,
    pub(crate) animation_frame: usize,
}

impl Default for SubagentActivitySnapshot {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            max_visible_rows: SUBAGENT_ACTIVITY_MAX_VISIBLE_ROWS,
            animation_frame: 0,
        }
    }
}

impl SubagentActivitySnapshot {
    pub(crate) fn visible_height(&self) -> u16 {
        if self.rows.is_empty() {
            return SUBAGENT_ACTIVITY_MIN_HEIGHT;
        }
        u16::try_from(self.rows.len().min(self.max_visible_rows)).unwrap_or(u16::MAX)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentActivityUpdate {
    pub(crate) agent_id: String,
    pub(crate) execution_id: Option<String>,
    pub(crate) label: String,
    pub(crate) phase: String,
    pub(crate) status: SubagentActivityStatus,
    pub(crate) elapsed: Option<Duration>,
    pub(crate) model: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) usage: Option<u64>,
    pub(crate) cache_hit_rate_basis_points: Option<u16>,
    pub(crate) label_from_task: bool,
}
