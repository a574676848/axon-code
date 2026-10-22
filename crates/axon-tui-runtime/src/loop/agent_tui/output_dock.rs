//! 工具输出 Dock 状态。

use crate::surface::{
    cell::{ROLE_TOOL, RenderCellRecord},
    render_policy::RenderPolicy,
    tool_output_buffer::ToolOutputView,
};

const OUTPUT_PAGE_STEP: u16 = 8;
const STREAM_STDERR: &str = "stderr";
const ERROR_TOKEN: &str = "error";

/// 工具输出 Dock 状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ToolOutputDock {
    title: String,
    lines: Vec<String>,
    scroll: u16,
}

impl ToolOutputDock {
    pub(super) fn open(&mut self, title: String, view: ToolOutputView) -> bool {
        if view.lines.is_empty() {
            return false;
        }
        self.title = title;
        self.lines = view.lines;
        self.scroll = 0;
        true
    }

    pub(crate) fn title(&self) -> &str {
        &self.title
    }

    pub(crate) fn lines(&self) -> &[String] {
        &self.lines
    }

    pub(crate) fn copy_text(&self) -> Option<String> {
        Some(self.lines.join("\n")).filter(|text| !text.trim().is_empty())
    }

    pub(crate) const fn scroll(&self) -> u16 {
        self.scroll
    }

    pub(super) fn scroll_up(&mut self) {
        self.scroll_up_by(OUTPUT_PAGE_STEP);
    }

    pub(super) fn scroll_down(&mut self) {
        self.scroll_down_by(OUTPUT_PAGE_STEP);
    }

    pub(super) fn scroll_up_by(&mut self, step: u16) {
        let max_scroll = u16::try_from(self.lines.len()).unwrap_or(u16::MAX);
        self.scroll = self.scroll.saturating_add(step).min(max_scroll);
    }

    pub(super) fn scroll_down_by(&mut self, step: u16) {
        self.scroll = self.scroll.saturating_sub(step);
    }

    pub(super) fn jump_top(&mut self) {
        self.scroll = u16::try_from(self.lines.len()).unwrap_or(u16::MAX);
    }

    pub(super) fn jump_bottom(&mut self) {
        self.scroll = 0;
    }

    pub(super) fn jump_error(&mut self) -> bool {
        let Some(index) = self
            .lines
            .iter()
            .position(|line| is_error_output_line(line))
        else {
            return false;
        };
        let bottom_offset = self.lines.len().saturating_sub(index + 1);
        self.scroll = u16::try_from(bottom_offset).unwrap_or(u16::MAX);
        true
    }
}

pub(super) fn selected_tool_cell_part_id(line: &RenderCellRecord) -> Option<String> {
    let decision = RenderPolicy::decide(line);
    if line.role != ROLE_TOOL || !decision.openable {
        return None;
    }
    line.semantic_part_id()
}

fn is_error_output_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with(STREAM_STDERR) || trimmed.to_ascii_lowercase().contains(ERROR_TOKEN)
}

#[cfg(test)]
mod tests {
    use super::{ToolOutputDock, selected_tool_cell_part_id};
    use crate::surface::{
        cell::{RenderCellAction, RenderCellRecord},
        cell_event_data::AgentRenderExecCell,
        tool_output_buffer::ToolOutputView,
    };

    #[test]
    fn selected_tool_cell_part_id_extracts_transcript_part_id() {
        let line = RenderCellRecord::exec_with_data(
            AgentRenderExecCell::running_with_id(
                "tool-part:test:42",
                "shell",
                "call-42",
                Some("{}".to_owned()),
            ),
            RenderCellAction::Run,
        );

        assert_eq!(
            selected_tool_cell_part_id(&line).as_deref(),
            Some("tool-part:test:42")
        );
    }

    #[test]
    fn output_dock_rejects_empty_output() {
        let mut dock = ToolOutputDock::default();

        assert!(!dock.open(
            "shell".to_owned(),
            ToolOutputView {
                lines: Vec::new(),
                truncated: false,
                omitted_lines: 0,
                total_bytes: 0,
                retained_bytes: 0,
                truncated_chunks: 0,
            }
        ));
        assert!(dock.copy_text().is_none());
    }

    #[test]
    fn output_dock_jumps_to_first_error_line() {
        let mut dock = ToolOutputDock::default();
        assert!(dock.open(
            "shell".to_owned(),
            ToolOutputView {
                lines: vec![
                    "stdout #001 │ ok".to_owned(),
                    "stderr #002 │ failed".to_owned(),
                    "stdout #003 │ after".to_owned(),
                ],
                truncated: false,
                omitted_lines: 0,
                total_bytes: 0,
                retained_bytes: 0,
                truncated_chunks: 0,
            }
        ));

        assert!(dock.jump_error());

        assert_eq!(dock.scroll(), 1);
    }

    #[test]
    fn output_dock_applies_custom_scroll_step() {
        let mut dock = ToolOutputDock::default();
        assert!(dock.open(
            "shell".to_owned(),
            ToolOutputView {
                lines: (0..12).map(|index| format!("line-{index}")).collect(),
                truncated: false,
                omitted_lines: 0,
                total_bytes: 0,
                retained_bytes: 0,
                truncated_chunks: 0,
            }
        ));

        dock.scroll_up_by(3);
        assert_eq!(dock.scroll(), 3);
        dock.scroll_down_by(3);
        assert_eq!(dock.scroll(), 0);
    }
}
