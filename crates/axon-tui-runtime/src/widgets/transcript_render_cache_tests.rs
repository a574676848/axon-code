use super::*;
use crate::surface::{
    cell::{RenderCellAction, RenderCellRecord, RenderReasoningCell},
    cell_event_data::{
        AgentRenderErrorCell, AgentRenderExecCell, AgentRenderExecCompletion,
        AgentRenderLifecycleStatus, AgentRenderMcpCell, AgentRenderMcpCellSource,
        AgentRenderMcpItem,
    },
    error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity},
};
use crate::widgets::transcript_cells::display_width;

#[test]
fn cache_metrics_reports_hits_and_misses_as_delta() {
    let cache = TranscriptRenderCache::default();
    let line = agent_cell("metrics", "缓存指标");

    let _ = cache.desired_height(&line, false, 80);
    let _ = cache.desired_height(&line, false, 80);

    let metrics = cache.take_metrics_delta();

    assert_eq!(metrics.hits, 1);
    assert_eq!(metrics.misses, 1);
    assert_eq!(metrics.render_profiler.cell_samples, 1);
    assert_eq!(metrics.render_profiler.rendered_cell_rows, 1);
    let idle_metrics = cache.take_metrics_delta();
    assert_eq!(idle_metrics.hits, 0);
    assert_eq!(idle_metrics.misses, 0);
    assert_eq!(
        idle_metrics.render_profiler,
        TuiRenderProfilerDelta::default()
    );
    assert_eq!(idle_metrics.current_entries, 1);
    assert!(idle_metrics.current_memory_bytes > 0);
}

#[test]
fn height_and_lines_share_one_cached_entry() {
    let cache = TranscriptRenderCache::default();
    let line = agent_cell("shared-entry", "一段需要缓存的 Markdown 文本");

    let _ = cache.render_cell_lines(&line, false, 40);
    let _ = cache.desired_height(&line, false, 40);

    assert_eq!(cache.entry_count(), 1);
}

#[test]
fn expanded_exec_uses_structured_detail_indentation() {
    let cache = TranscriptRenderCache::default();
    let exec = AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
        id: "exec:cache-detail".to_owned(),
        tool_name: "builtin.read_file".to_owned(),
        status: "completed".to_owned(),
        provider_call_id: "tool-1".to_owned(),
        summary: Some("读取完成".to_owned()),
        arguments_preview: Some(r#"{"path":"E:\\项目 资料\\说明.md"}"#.to_owned()),
        output_preview: Some("read_file path=说明.md lines=1-20".to_owned()),
        duration_ms: Some(1_250),
    });
    let line = RenderCellRecord::exec_with_data(exec, RenderCellAction::Ran);

    let rendered = cache.render_cell_lines(&line, true, 80);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert!(text[0].starts_with("  • "));
    assert!(text[0].contains("点击收起"));
    assert!(text.iter().skip(1).all(|line| line.starts_with("  ")));
    assert!(!text.join("\n").contains("builtin.read_file 完成"));
}

#[test]
fn expanded_mcp_keeps_structured_indentation_arguments_and_output() {
    let cache = TranscriptRenderCache::default();
    let mut item = AgentRenderMcpItem::catalog(
        "morph_mcp",
        Some("codebase_search".to_owned()),
        AgentRenderLifecycleStatus::Completed,
        None,
    );
    item.provider_call_id = Some("call-mcp-expanded".to_owned());
    item.arguments_preview = Some(r#"{"repo_path":"E:\\project"}"#.to_owned());
    item.output_preview = Some("真实结果第一行\n真实结果第二行".to_owned());
    let line = RenderCellRecord::mcp_with_data(AgentRenderMcpCell {
        id: "mcp:cache-expanded".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP 调用".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![item],
    });

    let rendered = cache.render_cell_lines(&line, true, 80);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert!(text[0].starts_with("  • MCP · morph_mcp.codebase_search"));
    assert!(text[0].contains("点击收起"));
    assert!(text.iter().skip(1).all(|line| line.starts_with("  ")));
    assert!(text.iter().any(|line| line.contains("参数")));
    assert!(text.iter().any(|line| line.contains("真实结果第一行")));
    assert!(text.iter().any(|line| line.contains("真实结果第二行")));
    assert_ne!(text, vec!["MCP"]);
}

#[test]
fn expanded_error_uses_structured_error_projection() {
    let cache = TranscriptRenderCache::default();
    let line = RenderCellRecord::error_with_data(AgentRenderErrorCell {
        id: "error:cache-expanded".to_owned(),
        title: "额度不足".to_owned(),
        message: "当前账号或模型额度不足。".to_owned(),
        details: vec!["代码 runtime_failed".to_owned(), "HTTP 403".to_owned()],
        next_step: None,
        severity: TuiErrorSeverity::UserFacing,
        action: TuiErrorAction::None,
        cell_kind: TuiErrorCellKind::Runtime,
    });

    let rendered = cache.render_cell_lines(&line, true, 80);
    let text = rendered.iter().map(line_text).collect::<Vec<_>>();

    assert!(text[0].starts_with("  ✗ "));
    assert!(text.iter().any(|line| line.starts_with("  │ ")));
    assert!(text.iter().any(|line| line.contains("HTTP 403")));
    assert!(!text.iter().any(|line| line == "额度不足"));
}

#[test]
fn cache_key_changes_for_revision_width_expanded_or_transcript_key() {
    let cache = TranscriptRenderCache::default();
    let mut line = agent_cell("revision", "第一版");

    let _ = cache.render_cell_lines(&line, false, 40);
    let _ = cache.render_cell_lines(&line, false, 40);
    assert_eq!(cache.entry_count(), 1);

    let _ = cache.render_cell_lines(&line, true, 40);
    let _ = cache.render_cell_lines(&line, false, 41);
    assert_eq!(cache.entry_count(), 2);

    let reasoning = RenderCellRecord::reasoning_with_data(RenderReasoningCell {
        id: "reasoning:cache-expanded".to_owned(),
        text: "可展开正文".to_owned(),
        done: true,
        elapsed_ms: Some(500),
    });
    let _ = cache.render_cell_lines(&reasoning, false, 40);
    let _ = cache.render_cell_lines(&reasoning, true, 40);
    assert_eq!(cache.entry_count(), 4);

    let render_id = line.render_id();
    line.replace_with(agent_cell("revision", "第二版"));
    assert_eq!(line.render_id(), render_id);

    let _ = cache.render_cell_lines(&line, false, 40);
    assert_eq!(cache.entry_count(), 5);
}

#[test]
fn collapsed_cell_reuses_cache_when_measured_again() {
    let cache = TranscriptRenderCache::default();
    let line = agent_cell("collapsed", "稳定内容");

    let _ = cache.render_cell_lines(&line, false, 40);
    let _ = cache.desired_height(&line, false, 40);

    assert_eq!(cache.entry_count(), 1);
}

#[test]
fn transcript_key_invalidates_streaming_cell_without_static_cache_pollution() {
    let cache = TranscriptRenderCache::default();
    let static_line = agent_cell("streaming-static", "静态内容");
    let mut streaming_line = streaming_agent_cell("streaming-active", "流式内容");

    let _ = cache.render_cell_lines(&static_line, false, 40);
    let _ = cache.render_cell_lines(&streaming_line, false, 40);
    streaming_line.bump_render_revision();
    let _ = cache.render_cell_lines(&streaming_line, false, 40);

    assert_eq!(static_line.transcript_key(), 0);
    assert_ne!(streaming_line.transcript_key(), 0);
    assert_eq!(cache.entry_count(), 3);
}

#[test]
fn animation_tick_invalidates_only_animation_cells() {
    let cache = TranscriptRenderCache::default();
    let static_line = agent_cell("animation-static", "静态内容");
    let mut animated_line = agent_cell("animation-active", "动画内容");

    let _ = cache.render_cell_lines(&static_line, false, 40);
    let _ = cache.render_cell_lines(&animated_line, false, 40);
    animated_line.set_transcript_animation_tick(Some(1));
    let _ = cache.render_cell_lines(&animated_line, false, 40);

    assert_eq!(static_line.transcript_key(), 0);
    assert_eq!(animated_line.transcript_key(), 1);
    assert_eq!(cache.entry_count(), 3);
}

#[test]
fn capacity_compaction_keeps_a_warm_cache() {
    let cache = TranscriptRenderCache::default();

    for index in 0..=MAX_CACHE_ENTRIES {
        let line = agent_cell(format!("capacity:{index}"), format!("line-{index}"));
        let _ = cache.render_cell_lines(&line, false, 80);
    }

    assert!(cache.entry_count() <= MAX_CACHE_ENTRIES);
    assert!(cache.entry_count() >= MAX_CACHE_ENTRIES / 2);
}

#[test]
fn desired_height_uses_renderer_physical_lines_without_secondary_wrap() {
    let cache = TranscriptRenderCache::default();
    let source = concat!(
        "```mermaid\n",
        "erDiagram\n",
        "    PROVIDER_DEFINITION ||--o{ ACTION_DEFINITION : contains actions for provider\n",
        "        string service PK\n",
        "```",
    );
    let line = agent_cell("physical-lines", source);

    let rendered = cache.render_cell_lines(&line, false, 32);
    let height = cache.desired_height(&line, false, 32);

    assert_eq!(usize::from(height), rendered.len());
    assert!(
        rendered
            .iter()
            .all(|line| display_width(&line_text(line)) <= 32)
    );
}

#[test]
fn resize_rerenders_source_for_each_exact_width() {
    let cache = TranscriptRenderCache::default();
    let source = concat!(
        "```mermaid\n",
        "erDiagram\n",
        "    PROVIDER_DEFINITION ||--o{ ACTION_DEFINITION : contains actions for provider and execution policy metadata\n",
        "        string service PK\n",
        "```",
    );
    let line = agent_cell("resize-exact-width", source);

    let rendered_80 = cache
        .render_cell_lines(&line, false, 80)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();
    let rendered_83 = cache
        .render_cell_lines(&line, false, 83)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();

    let metrics = cache.take_metrics_delta();
    assert_eq!(cache.entry_count(), 2);
    assert_eq!(metrics.hits, 0);
    assert_eq!(metrics.misses, 2);
    assert_ne!(rendered_80, rendered_83);
    assert!(rendered_80.iter().all(|line| display_width(line) <= 80));
    assert!(rendered_83.iter().all(|line| display_width(line) <= 83));
}

#[test]
fn cache_tracks_memory_usage_in_metrics() {
    let cache = TranscriptRenderCache::default();
    let line = agent_cell("memory-test", "一些需要缓存的内容");

    let _ = cache.render_cell_lines(&line, false, 80);
    let metrics = cache.take_metrics_delta();

    assert_eq!(metrics.current_entries, 1);
    assert!(metrics.current_memory_bytes > 0);
    assert!(metrics.max_entry_bytes > 0);
}

#[test]
fn cache_evicts_by_count_when_entry_limit_exceeded() {
    let cache = TranscriptRenderCache::default();

    for i in 0..(MAX_CACHE_ENTRIES + 100) {
        let line = agent_cell(format!("entry-{i}"), format!("text-{i}"));
        let _ = cache.render_cell_lines(&line, false, 80);
    }

    let metrics = cache.take_metrics_delta();

    assert!(metrics.current_entries <= MAX_CACHE_ENTRIES as u64);
    assert!(metrics.evictions_by_count > 0);
}

#[test]
fn cache_resize_updates_memory_correctly() {
    let cache = TranscriptRenderCache::default();
    let line = agent_cell("resize-memory", "内容");

    for width in [60, 70, 80, 90, 100] {
        let _ = cache.render_cell_lines(&line, false, width);
    }

    let before_metrics = cache.take_metrics_delta();
    let before_memory = before_metrics.current_memory_bytes;

    let _ = cache.compact_for_resize(80);

    let after_metrics = cache.take_metrics_delta();
    let after_memory = after_metrics.current_memory_bytes;

    assert!(after_memory < before_memory);
    assert!(after_metrics.current_entries < before_metrics.current_entries);
}

fn agent_cell(id: impl Into<String>, text: impl Into<String>) -> RenderCellRecord {
    RenderCellRecord::agent_message(format!("assistant:test:{}", id.into()), text)
}

fn streaming_agent_cell(id: impl Into<String>, text: impl Into<String>) -> RenderCellRecord {
    RenderCellRecord::streaming_agent_message(format!("assistant:test:{}", id.into()), text)
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}
