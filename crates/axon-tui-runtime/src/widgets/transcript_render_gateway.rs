//! RenderViewport cell 的唯一渲染入口。

use crate::{
    surface::{
        cell::{RenderCellMode, RenderCellRecord},
        render_policy::RenderPolicy,
    },
    widgets::{
        terminal_hyperlinks::{HyperlinkLine, plain_hyperlink_lines},
        transcript_cell_dispatch::render_structured_hyperlink_lines,
        transcript_cell_message,
        transcript_cells::render_startup_lines,
    },
};

pub(crate) struct TranscriptRenderGateway;

impl TranscriptRenderGateway {
    pub(crate) fn render_cell_hyperlink_lines(
        line: &RenderCellRecord,
        selected: bool,
        width: u16,
    ) -> Vec<HyperlinkLine> {
        match line.mode {
            RenderCellMode::Startup => {
                plain_hyperlink_lines(render_startup_lines(&line.text, width))
            }
            RenderCellMode::Cell(kind) => render_structured_hyperlink_lines(
                kind,
                selected,
                RenderPolicy::decide(line),
                line,
                width,
            ),
            RenderCellMode::Line => {
                transcript_cell_message::render_hyperlink_lines(selected, line, width)
            }
        }
    }
}
