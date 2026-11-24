//! Axon TUI 可复用 Widget 组件库。
//!
//! 基于 ratatui 构建的产品级 UI 组件，提供一致的视觉语言和交互模式。

mod adaptive_wrap;
mod bottom_view;
mod cells {
    pub(crate) mod error_cell;
}
mod command_palette;
mod composer;
mod composer_dock;
mod composer_popup_palette;
mod dock_render_gateway;
mod header_bar;
mod history_cell;
mod shimmer;
mod shortcut_help_dock;
mod terminal_hyperlinks;
mod terminal_local_links;
mod tool_output_dock;
mod transcript;
mod transcript_cell_dispatch;
mod transcript_cell_message;
mod transcript_cells;
mod transcript_command;
mod transcript_error;
mod transcript_exec;
mod transcript_exec_process;
mod transcript_file_change;
mod transcript_generation_status;
mod transcript_history;
mod transcript_hook;
mod transcript_markdown;
mod transcript_markdown_blocks;
mod transcript_markdown_code;
#[cfg(test)]
mod transcript_markdown_codex_tests;
mod transcript_markdown_helpers;
mod transcript_markdown_layout;
mod transcript_markdown_links;
#[cfg(test)]
mod transcript_markdown_mermaid_tests;
mod transcript_markdown_styles;
mod transcript_markdown_table;
mod transcript_markdown_table_layout;
#[cfg(test)]
mod transcript_markdown_table_tests;
#[cfg(test)]
mod transcript_markdown_test_support;
#[cfg(test)]
mod transcript_markdown_tests;
mod transcript_markdown_text_merge;
mod transcript_markdown_wrap;
mod transcript_mcp;
#[cfg(test)]
mod transcript_mcp_detail_tests;
mod transcript_patch_files;
#[cfg(test)]
mod transcript_patch_files_tests;
mod transcript_plan;
mod transcript_question;
mod transcript_reasoning;
mod transcript_render_cache;
mod transcript_render_gateway;
mod transcript_render_layout;
mod transcript_review;
mod transcript_status;
#[cfg(test)]
mod transcript_streaming_markdown;
mod transcript_style;
mod transcript_turn_summary;
mod transcript_usage;
mod transcript_viewport;
#[cfg(test)]
mod transcript_visual_contract_tests;
#[cfg(test)]
mod unicode_boundary_tests;
#[cfg(test)]
mod unicode_width_contract_tests;

pub(crate) use crate::surface::height_index::LiveTranscriptTail;
pub(crate) use bottom_view::BottomPaneView;
pub(crate) use command_palette::CommandPalette;
pub(crate) use composer::{
    Composer, ComposerDraft, ComposerDraftElement, ComposerDraftElementKind,
};
pub(crate) use composer_dock::ComposerDock;
pub(crate) use composer_popup_palette::ComposerPopupPalette;
pub(crate) use dock_render_gateway::DockRenderGateway;
pub(crate) use header_bar::HeaderBar;
pub(crate) use shortcut_help_dock::ShortcutHelpDockWidget;
pub(crate) use terminal_hyperlinks::{
    HyperlinkLine, TerminalHyperlinkSurface, terminal_frame_diff, visible_lines,
};
pub(crate) use tool_output_dock::ToolOutputDockWidget;
#[cfg(test)]
pub(crate) use transcript::transcript_view_metrics;
pub(crate) use transcript::transcript_view_metrics_with_height_cache;
pub(crate) use transcript_cell_message::{
    render_message_source_continuation_hyperlink_lines, render_message_source_hyperlink_lines,
};
pub(crate) use transcript_markdown::markdown_parser_options;
pub(crate) use transcript_render_cache::TranscriptRenderCache;
pub(crate) use transcript_render_gateway::TranscriptRenderGateway;
#[cfg(test)]
pub(crate) use transcript_streaming_markdown::render_streaming_markdown_lines;
pub(crate) use transcript_viewport::TranscriptViewportMetrics;
pub(crate) use transcript_viewport::{
    TranscriptViewportRequest, build_transcript_viewport_height_index,
    render_transcript_viewport_with_height_index, transcript_viewport_metrics_from_height_index,
};
