use std::sync::Arc;

use ratatui::text::Line;

use super::{
    HyperlinkLine, MAX_CACHE_ENTRIES, MAX_CACHE_MEMORY_BYTES, TranscriptRenderCache,
    test_support::agent_cell,
};
use crate::{
    surface::{cell::RenderCell, render_cache::CellCacheKey},
    widgets::terminal_hyperlinks::TerminalHyperlink,
};

#[test]
fn cache_rejects_an_oversized_entry_and_removes_the_replaced_key() {
    let cache = TranscriptRenderCache::default();
    let cell = agent_cell("oversized-entry", "正常布局");
    let key = CellCacheKey::from_cell(&cell, false, 80);
    let normal_layout = Arc::from(vec![HyperlinkLine {
        line: Line::raw("正常布局"),
        hyperlinks: Vec::new(),
    }]);
    let _ = cache.insert_rendered(key, normal_layout);
    assert_eq!(cache.entry_count(), 1);

    let mut oversized_destination = String::with_capacity(MAX_CACHE_MEMORY_BYTES);
    oversized_destination.push('x');
    let oversized_layout = Arc::from(vec![HyperlinkLine {
        line: Line::raw("x"),
        hyperlinks: vec![TerminalHyperlink {
            columns: 0..1,
            destination: oversized_destination,
        }],
    }]);
    let _ = cache.insert_rendered(key, oversized_layout);

    let metrics = cache.take_metrics_delta();
    assert_eq!(cache.entry_count(), 0);
    assert_eq!(metrics.current_entries, 0);
    assert_eq!(metrics.current_memory_bytes, 0);
    assert!(metrics.max_entry_bytes > MAX_CACHE_MEMORY_BYTES as u64);
}

#[test]
fn cache_evicts_by_memory_when_budget_exceeded() {
    let cache = TranscriptRenderCache::default();

    let large_destination = "x".repeat(1024 * 1024);
    for index in 0..80 {
        let key = CellCacheKey::from_cell(&agent_cell(format!("large-{index}"), "x"), false, 80);
        let line = HyperlinkLine {
            line: Line::raw("x"),
            hyperlinks: vec![TerminalHyperlink {
                columns: 0..1,
                destination: large_destination.clone(),
            }],
        };
        let _ = cache.insert_rendered(key, Arc::from(vec![line]));
    }

    let metrics = cache.take_metrics_delta();

    assert!(metrics.current_entries < 80);
    assert!(metrics.current_memory_bytes < MAX_CACHE_MEMORY_BYTES as u64);
    assert!(metrics.evictions_by_memory > 0);
}

#[test]
fn count_eviction_keeps_existing_entries_when_large_layout_fits_memory_budget() {
    let cache = TranscriptRenderCache::default();
    for index in 0..MAX_CACHE_ENTRIES {
        let key = CellCacheKey::from_cell(&agent_cell(format!("small-{index}"), "x"), false, 80);
        let _ = cache.insert_rendered(
            key,
            Arc::from(vec![HyperlinkLine {
                line: Line::raw("x"),
                hyperlinks: Vec::new(),
            }]),
        );
    }

    let key = CellCacheKey::from_cell(&agent_cell("large-but-valid", "x"), false, 80);
    let _ = cache.insert_rendered(
        key,
        Arc::from(vec![HyperlinkLine {
            line: Line::raw("x"),
            hyperlinks: vec![TerminalHyperlink {
                columns: 0..1,
                destination: "x".repeat(MAX_CACHE_MEMORY_BYTES / 2),
            }],
        }]),
    );

    let metrics = cache.take_metrics_delta();

    assert_eq!(
        cache.entry_count(),
        MAX_CACHE_ENTRIES.saturating_mul(3) / 4 + 1
    );
    assert!(metrics.evictions_by_count > 0);
    assert_eq!(metrics.evictions_by_memory, 0);
}

#[test]
fn removing_history_cell_reclaims_all_cached_layout_revisions() {
    let cache = TranscriptRenderCache::default();
    let mut cell = agent_cell("evicted", "第一版");
    let cell_id = cell.id();

    let _ = cache.render_cell_hyperlink_lines(&cell, false, 80);
    let _ = cache.render_cell_hyperlink_lines(&cell, false, 81);
    cell.replace_with(agent_cell("evicted", "第二版"));
    let _ = cache.render_cell_hyperlink_lines(&cell, false, 80);

    assert_eq!(cache.entry_count(), 3);
    cache.remove_cells(&[cell_id]);

    let metrics = cache.take_metrics_delta();
    assert_eq!(cache.entry_count(), 0);
    assert_eq!(metrics.current_entries, 0);
    assert_eq!(metrics.current_memory_bytes, 0);
}
