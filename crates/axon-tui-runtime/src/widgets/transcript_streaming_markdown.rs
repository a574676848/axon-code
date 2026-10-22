//! 流式回复的 Markdown 展示入口。
//!
//! streaming controller 仍以原始 source 做最终合并，但活动尾部与最终消息共享
//! source-backed renderer，确保 resize、列表和表格的物理行完全一致。

use ratatui::text::Line;

use crate::{
    surface::cell::ROLE_AGENT,
    widgets::{
        terminal_hyperlinks::visible_lines,
        transcript_cell_message::render_message_source_hyperlink_lines,
    },
};

pub(crate) fn render_streaming_markdown_lines(source: &str, width: u16) -> Vec<Line<'static>> {
    visible_lines(&render_message_source_hyperlink_lines(
        source, ROLE_AGENT, false, width,
    ))
}
