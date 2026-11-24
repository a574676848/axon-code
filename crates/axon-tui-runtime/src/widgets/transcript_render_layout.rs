//! Transcript cache 的最终物理行布局。

use crate::surface::{
    cell::{RenderCell, RenderCellData, RenderCellRecord},
    height_index::RenderHeightProvider,
};

use super::{
    TranscriptRenderCache, TranscriptRenderGateway,
    history_cell::HistoryCell,
    terminal_hyperlinks::{HyperlinkLine, plain_hyperlink_lines},
    transcript_markdown_wrap::wrap_lines_without_prefix,
};

pub(super) fn render_cell_record(cell: &dyn RenderCell) -> &RenderCellRecord {
    cell.as_any()
        .downcast_ref::<RenderCellRecord>()
        .expect("TranscriptRenderCache 只接受 RenderCellRecord 渲染输入")
}

pub(super) fn render_record_for_cache(
    line: &RenderCellRecord,
    expanded: bool,
    width: u16,
) -> Vec<HyperlinkLine> {
    if expanded
        && matches!(
            line.data.as_ref(),
            Some(
                RenderCellData::Diff(_)
                    | RenderCellData::Exec(_)
                    | RenderCellData::Mcp(_)
                    | RenderCellData::Reasoning(_)
                    | RenderCellData::Error(_)
            )
        )
    {
        TranscriptRenderGateway::render_cell_hyperlink_lines(line, true, width)
    } else if expanded {
        wrap_lines_without_prefix(
            plain_hyperlink_lines(line.raw_lines()),
            usize::from(width.max(1)),
        )
    } else {
        TranscriptRenderGateway::render_cell_hyperlink_lines(line, false, width)
    }
}

pub(super) fn physical_lines_height(lines: &[HyperlinkLine]) -> u16 {
    lines.len().try_into().unwrap_or(u16::MAX)
}

impl RenderHeightProvider for TranscriptRenderCache {
    fn desired_height(&self, cell: &dyn RenderCell, expanded: bool, width: u16) -> u16 {
        TranscriptRenderCache::desired_height(self, cell, expanded, width)
    }
}
