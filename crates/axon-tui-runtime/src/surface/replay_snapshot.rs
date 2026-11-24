//! Resume 会话快照回放的 UX 状态。

use crate::{
    AgentTuiLifecyclePhase,
    surface::cell::{RenderCellRecord, RenderStatusCell, RenderStatusItem},
};

pub(crate) const RESUME_LOADING_TITLE: &str = "正在加载会话历史...";
const RESUME_FAILED_TITLE: &str = "会话历史加载失败";
const RESUME_STATUS_LABEL: &str = "恢复中...";
const RESUME_INPUT_AVAILABLE_LABEL: &str = "输入可用";
const RESUME_INPUT_BLOCKED_LABEL: &str = "输入等待中";
const RESUME_LOADED_KIND: &str = "resume.loaded_cells";
const RESUME_INPUT_KIND: &str = "resume.input";
const RESUME_STATUS_PART_ID: &str = "status:resume:snapshot";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ReplaySnapshotPhase {
    Loading,
    Completed,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ReplaySnapshotHeaderState {
    pub(crate) phase: ReplaySnapshotPhase,
    pub(crate) loaded_cells: usize,
    pub(crate) input_available: bool,
}

impl ReplaySnapshotHeaderState {
    pub(crate) const fn loading(loaded_cells: usize, input_available: bool) -> Self {
        Self {
            phase: ReplaySnapshotPhase::Loading,
            loaded_cells,
            input_available,
        }
    }

    pub(crate) const fn completed(loaded_cells: usize, input_available: bool) -> Self {
        Self {
            phase: ReplaySnapshotPhase::Completed,
            loaded_cells,
            input_available,
        }
    }

    pub(crate) const fn error(loaded_cells: usize, input_available: bool) -> Self {
        Self {
            phase: ReplaySnapshotPhase::Error,
            loaded_cells,
            input_available,
        }
    }

    pub(crate) const fn is_loading(self) -> bool {
        matches!(self.phase, ReplaySnapshotPhase::Loading)
    }

    pub(crate) const fn status_label(self) -> Option<&'static str> {
        match self.phase {
            ReplaySnapshotPhase::Loading => Some(RESUME_STATUS_LABEL),
            ReplaySnapshotPhase::Completed | ReplaySnapshotPhase::Error => None,
        }
    }

    pub(crate) fn loaded_cells_label(self) -> String {
        loaded_cells_label(self.loaded_cells)
    }

    pub(crate) const fn input_label(self) -> &'static str {
        if self.input_available {
            RESUME_INPUT_AVAILABLE_LABEL
        } else {
            RESUME_INPUT_BLOCKED_LABEL
        }
    }
}

pub(crate) fn replay_loading_cell(loaded_cells: usize) -> RenderCellRecord {
    resume_status_cell(RESUME_LOADING_TITLE, loaded_cells)
}

pub(crate) fn replay_failed_cell(loaded_cells: usize) -> RenderCellRecord {
    resume_status_cell(RESUME_FAILED_TITLE, loaded_cells)
}

fn resume_status_cell(title: &str, loaded_cells: usize) -> RenderCellRecord {
    RenderCellRecord::status_with_data(RenderStatusCell {
        id: RESUME_STATUS_PART_ID.to_owned(),
        title: title.to_owned(),
        items: vec![
            RenderStatusItem {
                kind: RESUME_LOADED_KIND.to_owned(),
                summary: loaded_cells_label(loaded_cells),
                phase: Some(AgentTuiLifecyclePhase::Notice),
                indicator: None,
                tool_name: None,
                duration_ms: None,
                turn_id: None,
                attempt_index: None,
            },
            RenderStatusItem {
                kind: RESUME_INPUT_KIND.to_owned(),
                summary: RESUME_INPUT_AVAILABLE_LABEL.to_owned(),
                phase: Some(AgentTuiLifecyclePhase::Notice),
                indicator: None,
                tool_name: None,
                duration_ms: None,
                turn_id: None,
                attempt_index: None,
            },
        ],
    })
}

fn loaded_cells_label(loaded_cells: usize) -> String {
    format!("{loaded_cells} cells 已加载")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_header_loading_keeps_input_available() {
        let state = ReplaySnapshotHeaderState::loading(7, true);

        assert_eq!(state.status_label(), Some("恢复中..."));
        assert_eq!(state.loaded_cells_label(), "7 cells 已加载");
        assert_eq!(state.input_label(), "输入可用");
    }
}
