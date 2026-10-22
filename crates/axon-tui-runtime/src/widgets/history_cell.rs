//! Codex 风格 HistoryCell 契约。
//!
//! HistoryCell 负责 transcript 行的显示模式、链接标注和文本投影。

use std::any::Any;

use pulldown_cmark::{Event, Parser};
use ratatui::text::Line;

use crate::r#loop::agent_tui::{
    display_units::normalize_display_units_in_text, duration_format::format_duration_millis_compact,
};
use crate::{
    surface::{
        cell::{RenderCell, RenderCellData, RenderCellMode, RenderCellRecord},
        cell_diff_data::{
            AgentRenderDiffCell, file_change_label, line_stats, raw_diff_lines, rendered_path,
        },
        cell_event_data::{AgentRenderExecCell, AgentRenderExecStatus},
        cell_plan_data::AgentRenderPlanCell,
        render_policy::RenderPolicy,
    },
    widgets::{
        TranscriptRenderGateway, markdown_parser_options,
        terminal_hyperlinks::{HyperlinkLine, plain_hyperlink_lines, visible_lines},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryRenderMode {
    Rich,
    Raw,
}

pub(crate) trait HistoryCell: std::fmt::Debug + Send + Sync + Any {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>>;

    fn raw_lines(&self) -> Vec<Line<'static>>;

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        plain_hyperlink_lines(self.display_lines(width))
    }

    fn display_lines_for_mode(&self, width: u16, mode: HistoryRenderMode) -> Vec<Line<'static>> {
        match mode {
            HistoryRenderMode::Rich => visible_lines(&self.display_hyperlink_lines(width)),
            HistoryRenderMode::Raw => self.raw_lines(),
        }
    }

    fn display_hyperlink_lines_for_mode(
        &self,
        width: u16,
        mode: HistoryRenderMode,
    ) -> Vec<HyperlinkLine> {
        match mode {
            HistoryRenderMode::Rich => self.display_hyperlink_lines(width),
            HistoryRenderMode::Raw => plain_hyperlink_lines(self.raw_lines()),
        }
    }

    fn desired_height_for_mode(&self, width: u16, mode: HistoryRenderMode) -> u16 {
        rendered_lines_height(&self.display_lines_for_mode(width, mode))
    }
}

impl HistoryCell for RenderCellRecord {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        visible_lines(&TranscriptRenderGateway::render_cell_hyperlink_lines(
            self, false, width,
        ))
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        raw_lines_for_agent_line(self)
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        TranscriptRenderGateway::render_cell_hyperlink_lines(self, false, width)
    }
}

pub(crate) fn render_cell_contract_probe(cell: &dyn RenderCell, width: u16) {
    let _ = cell.kind();
    let _ = cell.payload_envelope();
    let _ = cell.summary();
    let _ = cell.is_stream_continuation();
    let _ = cell.transcript_animation_tick();
    let _ = cell.provider_call_id();
    let _ = RenderPolicy::decide(cell);
    let _ = cell.transcript_key();
    let _ = cell.as_any();
    if let Some(record) = cell.as_any().downcast_ref::<RenderCellRecord>() {
        let _ = record.desired_height_for_mode(width, HistoryRenderMode::Rich);
        let _ = record.raw_lines();
        let _ = record.display_hyperlink_lines_for_mode(width, HistoryRenderMode::Raw);
    }
}

fn rendered_lines_height(lines: &[Line<'_>]) -> u16 {
    lines.len().try_into().unwrap_or(u16::MAX)
}

fn raw_lines_for_agent_line(line: &RenderCellRecord) -> Vec<Line<'static>> {
    match (&line.mode, &line.data) {
        (_, Some(RenderCellData::Message(_))) => plain_text_lines(&line.text),
        (_, Some(RenderCellData::Plan(cell))) => plan_raw_lines(cell),
        (_, Some(RenderCellData::Exec(cell))) => exec_raw_lines(cell),
        (_, Some(RenderCellData::Diff(cell))) => diff_raw_lines(cell),
        (_, Some(RenderCellData::TurnSummary(_))) => Vec::new(),
        (RenderCellMode::Startup, _) | (RenderCellMode::Line, _) | (RenderCellMode::Cell(_), _) => {
            plain_source_lines(&line.text)
        }
    }
}

fn plan_raw_lines(cell: &AgentRenderPlanCell) -> Vec<Line<'static>> {
    plain_text_lines(&cell.source)
}

fn exec_raw_lines(cell: &AgentRenderExecCell) -> Vec<Line<'static>> {
    let mut lines = vec![format!(
        "{} {}",
        cell.tool_name,
        exec_status_label(cell.status)
    )];
    if let Some(arguments) = non_empty(cell.arguments_preview.as_deref()) {
        lines.push(format!("参数: {arguments}"));
    }
    if let Some(output) = non_empty(cell.output_preview.as_deref()) {
        lines.push(format!("输出: {output}"));
    }
    if let Some(summary) = non_empty(cell.summary.as_deref()) {
        lines.push(format!("摘要: {summary}"));
    }
    if let Some(duration_ms) = cell.duration_ms {
        lines.push(format!(
            "耗时: {}",
            format_duration_millis_compact(duration_ms)
        ));
    }
    lines.into_iter().map(Line::from).collect()
}

fn diff_raw_lines(cell: &AgentRenderDiffCell) -> Vec<Line<'static>> {
    let mut lines = vec![cell.title.clone()];
    for (path, change) in &cell.changes {
        let (added, removed) = line_stats(change);
        lines.push(format!(
            "{} {} (+{} -{})",
            file_change_label(change),
            rendered_path(path, change),
            added,
            removed
        ));
        if let Some(diagnostics) = cell.diagnostics.get(path) {
            lines.extend(diagnostics.iter().map(|item| format!("诊断: {item}")));
        }
        lines.extend(raw_diff_lines(change));
    }
    lines.into_iter().map(Line::from).collect()
}

fn plain_text_lines(source: &str) -> Vec<Line<'static>> {
    let mut text = String::new();
    for event in Parser::new_ext(source, markdown_parser_options()) {
        match event {
            Event::Text(value) | Event::Code(value) => text.push_str(&value),
            Event::SoftBreak | Event::HardBreak => text.push('\n'),
            Event::TaskListMarker(_) => {}
            _ => {}
        }
    }
    plain_source_lines(&text)
}

fn plain_source_lines(source: &str) -> Vec<Line<'static>> {
    normalize_display_units_in_text(source)
        .lines()
        .map(|line| Line::from(line.to_owned()))
        .collect()
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

const fn exec_status_label(status: AgentRenderExecStatus) -> &'static str {
    match status {
        AgentRenderExecStatus::Running => "运行中",
        AgentRenderExecStatus::Completed => "完成",
        AgentRenderExecStatus::Failed => "失败",
        AgentRenderExecStatus::Output => "输出",
    }
}

#[cfg(test)]
mod tests {
    use super::{HistoryCell, HistoryRenderMode};
    use crate::app_shell::style_tokens::MESSAGE_INITIAL_PREFIX;
    use crate::surface::cell::RenderCellRecord;

    #[test]
    fn raw_mode_uses_semantic_text_without_rich_prefix_or_markdown_markers() {
        let line = RenderCellRecord::agent_message(
            "assistant:test:raw-history",
            "### 标题\n请查看 **结果**",
        );

        let rich = lines_text(line.display_lines_for_mode(80, HistoryRenderMode::Rich));
        let raw = lines_text(line.display_lines_for_mode(80, HistoryRenderMode::Raw));

        assert!(rich.starts_with(MESSAGE_INITIAL_PREFIX));
        assert_ne!(rich, raw);
        assert!(raw.contains("标题"));
        assert!(raw.contains("结果"));
        assert!(!raw.contains(MESSAGE_INITIAL_PREFIX));
        assert!(!raw.contains("###"));
        assert!(!raw.contains("**"));
    }

    #[test]
    fn raw_mode_normalizes_duration_fields() {
        let line = RenderCellRecord::system("tool result duration_ms=65000 elapsed_ms: 1250");

        let raw = lines_text(line.display_lines_for_mode(80, HistoryRenderMode::Raw));

        assert!(raw.contains("duration_ms=1m 05s"));
        assert!(raw.contains("elapsed_ms: 1.25s"));
        assert!(!raw.contains("duration_ms=65000"));
    }

    fn lines_text(lines: Vec<ratatui::text::Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}
