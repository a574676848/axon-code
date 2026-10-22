//! Runtime History transcript cell 生命周期编排。

use crate::r#loop::agent_tui::{
    run_output::{AgentTuiHistoryItem, AgentTuiHistoryLine, AgentTuiLifecycleStatusView},
    state::AgentTuiState,
};
use crate::surface::{cell::RenderCellRecord, cell_event_data::AgentRenderHistoryCell};

impl AgentTuiState {
    pub(in crate::r#loop::agent_tui) fn upsert_history_cell(&mut self, line: AgentTuiHistoryLine) {
        let line =
            RenderCellRecord::history_with_data(AgentRenderHistoryCell::from_tui_line(&line));
        let Some(part_id) = line.semantic_part_id() else {
            return;
        };
        let existing_index = {
            let part_id = part_id.as_str();
            self.cells
                .transcript
                .iter()
                .position(|candidate| candidate.semantic_part_id().as_deref() == Some(part_id))
        };
        if let Some(index) = existing_index {
            let _ = self.cells.transcript.replace_cell(index, line);
            return;
        }
        self.cells.transcript.push(line);
    }

    pub(in crate::r#loop::agent_tui) fn push_history_lifecycle_status(
        &mut self,
        view: &AgentTuiLifecycleStatusView,
    ) -> bool {
        let Some(line) = history_status_line(view) else {
            return false;
        };
        let Some(part_id) = line.semantic_part_id() else {
            return false;
        };
        if history_lifecycle_is_terminal(view) {
            if let Some(active_part) = self
                .cells
                .active_runtime_cells
                .take_history_lifecycle_cell(&part_id)
                && let Some(existing) = self.cells.transcript.get_mut(active_part.index)
            {
                existing.replace_with(line);
                return true;
            }
            self.cells.transcript.push(line);
            return true;
        }
        if let Some(active_part) = self
            .cells
            .active_runtime_cells
            .take_history_lifecycle_cell(&part_id)
            && let Some(existing) = self.cells.transcript.get_mut(active_part.index)
        {
            existing.replace_with(line);
            self.cells
                .active_runtime_cells
                .set_history_lifecycle_cell(part_id, active_part.index);
            return true;
        }
        self.cells.transcript.push(line);
        self.cells
            .active_runtime_cells
            .set_history_lifecycle_cell(part_id, self.cells.transcript.len().saturating_sub(1));
        true
    }
}

fn history_status_line(view: &AgentTuiLifecycleStatusView) -> Option<RenderCellRecord> {
    let status_label = match view.kind.as_str() {
        "CompactionStarted" => "进行中",
        "CompactionCompleted" => "完成",
        // 失败与空闲排期同样是压缩的可见事实：phase 为 history 时不会落入
        // 通用 lifecycle 通道，缺少映射就会被整条丢弃。
        "CompactionFailed" => "失败",
        "CompactionPlanned" => "已排期",
        _ => return None,
    };
    let line = AgentTuiHistoryLine::with_id(
        format!("history:lifecycle:{}", view.id),
        "历史",
        vec![AgentTuiHistoryItem::new(
            view.summary.clone(),
            None,
            Some(status_label.to_owned()),
        )],
    );
    Some(RenderCellRecord::history_with_data(
        AgentRenderHistoryCell::from_tui_line(&line),
    ))
}

fn history_lifecycle_is_terminal(view: &AgentTuiLifecycleStatusView) -> bool {
    matches!(
        view.kind.as_str(),
        "CompactionCompleted" | "CompactionFailed" | "CompactionPlanned"
    )
}
