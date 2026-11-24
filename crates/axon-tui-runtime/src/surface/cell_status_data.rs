//! 状态类渲染单元事实结构。

use crate::{
    AgentTuiLifecycleLine, AgentTuiLifecyclePhase, AgentTuiLifecycleStatusView, AgentTuiStatusItem,
    AgentTuiStatusLine,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderStatusCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) items: Vec<RenderStatusItem>,
}

impl RenderStatusCell {
    pub(crate) fn from_tui_status(status: &AgentTuiStatusLine) -> Self {
        Self {
            id: status.id.clone(),
            title: status.title.clone(),
            items: tui_status_items(status),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderCommandCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) items: Vec<RenderStatusItem>,
}

impl RenderCommandCell {
    pub(crate) fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        items: Vec<RenderStatusItem>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            items,
        }
    }

    pub(crate) fn from_tui_status(status: &AgentTuiStatusLine) -> Self {
        Self {
            id: status.id.clone(),
            title: status.title.clone(),
            items: tui_status_items(status),
        }
    }
}

/// 状态行条目投影：`RenderStatusCell` 与 `RenderCommandCell` 共用同一逐项口径。
fn tui_status_items(status: &AgentTuiStatusLine) -> Vec<RenderStatusItem> {
    status
        .items
        .iter()
        .map(RenderStatusItem::from_tui_status_item)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderGenerationStatusCell {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) items: Vec<RenderLifecycleItem>,
}

impl RenderGenerationStatusCell {
    pub(crate) fn from_lifecycle_line(line: &AgentTuiLifecycleLine) -> Self {
        Self {
            id: line.id.clone(),
            title: line.title.clone(),
            items: line
                .items
                .iter()
                .map(RenderLifecycleItem::from_tui_lifecycle_view)
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderStatusItem {
    pub(crate) kind: String,
    pub(crate) summary: String,
    pub(crate) phase: Option<AgentTuiLifecyclePhase>,
    pub(crate) indicator: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) turn_id: Option<u64>,
    pub(crate) attempt_index: Option<usize>,
}

impl RenderStatusItem {
    pub(crate) fn from_tui_status_item(item: &AgentTuiStatusItem) -> Self {
        Self {
            kind: item.kind.clone(),
            summary: item.summary.clone(),
            phase: item.phase,
            indicator: None,
            tool_name: item.tool_name.clone(),
            duration_ms: item.duration_ms,
            turn_id: item.turn_id,
            attempt_index: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderLifecycleItem {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) summary: String,
    pub(crate) phase: AgentTuiLifecyclePhase,
    pub(crate) indicator: Option<String>,
    pub(crate) tool_name: Option<String>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) attempt_index: Option<usize>,
}

impl RenderLifecycleItem {
    pub(crate) fn from_tui_lifecycle_view(view: &AgentTuiLifecycleStatusView) -> Self {
        Self {
            id: view.id.clone(),
            kind: view.kind.clone(),
            summary: view.summary.clone(),
            phase: view.phase,
            indicator: None,
            tool_name: view.tool_name.clone(),
            duration_ms: view.duration_ms,
            attempt_index: view.attempt_index,
        }
    }
}
