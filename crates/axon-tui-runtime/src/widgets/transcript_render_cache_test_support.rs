use std::sync::Arc;

use ratatui::text::Line;

use crate::surface::cell::RenderCellRecord;
use crate::widgets::terminal_hyperlinks::HyperlinkLine;

use super::TranscriptRenderCache;

pub(super) fn agent_cell(id: impl Into<String>, text: impl Into<String>) -> RenderCellRecord {
    RenderCellRecord::agent_message(format!("assistant:test:{}", id.into()), text)
}

pub(super) fn streaming_agent_cell(
    id: impl Into<String>,
    text: impl Into<String>,
) -> RenderCellRecord {
    RenderCellRecord::streaming_agent_message(format!("assistant:test:{}", id.into()), text)
}

pub(super) fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn primed_streaming_layout_is_shared_by_height_and_viewport_without_memory_growth() {
    let cache = TranscriptRenderCache::default();
    let line = streaming_agent_cell("primed", "不应重新解析的原文");
    let layout = Arc::from(vec![HyperlinkLine::new(Line::raw("• controller 预布局"))]);

    cache.prime_streaming_layout(&line, 80, Arc::clone(&layout));
    let initial_memory = cache.take_metrics_delta().current_memory_bytes;
    cache.prime_streaming_layout(&line, 80, Arc::clone(&layout));
    let rendered = cache.render_cell_hyperlink_lines(&line, false, 80);

    assert!(Arc::ptr_eq(&layout, &rendered));
    assert_eq!(line_text(&rendered[0].line), "• controller 预布局");
    assert_eq!(cache.desired_height(&line, false, 80), 1);
    assert_eq!(cache.entry_count(), 1);
    assert_eq!(
        cache.take_metrics_delta().current_memory_bytes,
        initial_memory
    );
}

#[test]
fn cache_hit_shares_precomputed_hyperlink_lines_without_copying() {
    let cache = TranscriptRenderCache::default();
    let line = agent_cell("shared-lines", "[链接](https://example.com)");

    let first = cache.render_cell_hyperlink_lines(&line, false, 80);
    let second = cache.render_cell_hyperlink_lines(&line, false, 80);

    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn streaming_preheat_discards_only_superseded_revisions() {
    let cache = TranscriptRenderCache::default();
    let static_line = agent_cell("static", "不应被流式更新驱逐");
    let mut streaming_line = streaming_agent_cell("active", "流式第一版");
    let layout = Arc::from(vec![HyperlinkLine::new(Line::raw("• 第一版"))]);

    let _ = cache.render_cell_hyperlink_lines(&static_line, false, 80);
    cache.prime_streaming_layout(&streaming_line, 80, Arc::clone(&layout));
    streaming_line.bump_render_revision();
    cache.prime_streaming_layout(&streaming_line, 80, layout);

    assert_eq!(cache.entry_count(), 2);
    assert_eq!(cache.take_metrics_delta().current_entries, 2);
}
