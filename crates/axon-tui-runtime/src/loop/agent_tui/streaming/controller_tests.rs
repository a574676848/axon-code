use super::*;
use crate::agent_tui::streaming::STREAM_QUEUED_LINE_CAPACITY;

#[test]
fn streams_stable_lines_and_tail_without_duplication() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("a\nb", now);

    assert_eq!(controller.queued_lines(), 1);
    assert_eq!(line_texts(&controller.current_tail_lines()), vec!["• b"]);
    let stable = controller.on_commit_tick().expect("stable cell");
    assert_eq!(stable.text, "a\nb");

    assert_eq!(controller.finalize().cell.expect("final cell").text, "a\nb");
}

#[test]
fn streaming_assistant_hides_memory_citation_markup_across_deltas() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("结论<oai-mem-cit", now);
    assert_eq!(controller.current_visible_text(), "结论");

    controller.push_delta(
        "ation><citation_entries>memory:7</citation_entries></oai-mem-citation>结束",
        now,
    );

    assert_eq!(controller.current_visible_text(), "结论结束");
    assert_eq!(
        controller.finalize().cell.expect("final cell").text,
        "结论结束"
    );
}

#[test]
fn holds_table_until_block_boundary_then_renders_structured_table() {
    const STREAM_WIDTH: u16 = 80;
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", STREAM_WIDTH, ".");

    controller.push_delta("intro\n", now);
    assert_eq!(
        controller.on_commit_tick().expect("stable intro").text,
        "intro\n"
    );

    for delta in [
        "| 问题 | 之前 | 修正 |\n",
        "| --- | --- | --- |\n",
        "| `axon resume` | 恢复持久化会话 | 保持会话选择语义 |\n",
        "| `axon mcp` | MCP 管理入口 | 支持 `list\\|get\\|add\\|remove` |\n",
    ] {
        controller.push_delta(delta, now);
        assert_eq!(controller.queued_lines(), 0, "表格闭合前不得提交物理行");
        assert!(
            controller.current_tail_lines().is_empty(),
            "未闭合表格不得以普通 Markdown 尾行泄露"
        );
    }

    controller.push_delta("\n", now);
    let committed = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("完整表格应在空行边界一次性提交");
    let grid = line_texts(&render_streaming_markdown_lines(
        &committed.text,
        STREAM_WIDTH,
    ))
    .join("\n");

    assert!(committed.text.contains("| --- | --- |"));
    assert!(
        grid.contains('┌') && grid.contains('└'),
        "实际渲染：\n{grid}"
    );
    assert!(!grid.contains("| --- | --- |"), "实际渲染：\n{grid}");
}

#[test]
fn keeps_potential_table_header_in_active_source_until_next_line_decides() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("intro\n| a | b |\n", now);

    assert_eq!(controller.queued_lines(), 1);
    let active_source = controller.current_active_source();
    assert!(active_source.contains("| a | b |"));
    let rendered = line_texts(&render_streaming_markdown_lines(&active_source, 80)).join("\n");
    assert!(rendered.contains('a') && rendered.contains('b'));
    assert!(
        !rendered.contains('┌'),
        "潜在表头不应提前形成表格边框：\n{rendered}"
    );
    assert!(controller.on_commit_tick().is_some());

    controller.push_delta("不是表格\n", now);

    let rendered = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("released paragraph")
        .text;
    assert!(rendered.contains("| a | b |"));
    assert!(rendered.contains("不是表格"));
}

#[test]
fn holds_fenced_code_until_closing_fence() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n```rust\nfn main() {}\n", now);

    assert_eq!(controller.queued_lines(), 1);
    assert!(controller.current_tail_lines().is_empty());
    assert_eq!(
        controller.on_commit_tick().expect("stable prefix").text,
        "before\n"
    );

    controller.push_delta("```\nafter\n", now);

    let rendered = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("stable code block")
        .text;
    assert!(rendered.contains("fn main() {}"));
    assert!(rendered.contains("```"), "cell 只保存 raw source");
}

#[test]
fn holds_list_until_blank_boundary() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n- item\n  continuation\n", now);

    assert_eq!(controller.queued_lines(), 1);
    assert!(controller.current_tail_lines().is_empty());
    assert_eq!(
        controller.on_commit_tick().expect("stable prefix").text,
        "before\n"
    );

    controller.push_delta("\n", now);

    let rendered = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("stable list")
        .text;
    assert!(rendered.contains("item"));
    assert!(rendered.contains("continuation"));
}

#[test]
fn releases_completed_list_items_while_holding_last_item() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n- first\n- second\n", now);

    let rendered = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("stable prefix and first item")
        .text;
    assert!(rendered.contains("before"));
    assert!(rendered.contains("first"));
    assert!(!rendered.contains("second"));
    assert!(controller.current_tail_lines().is_empty());
}

#[test]
fn holds_html_until_blank_boundary() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n<div>\ncontent\n", now);

    assert_eq!(controller.queued_lines(), 1);
    assert!(controller.current_tail_lines().is_empty());
    assert_eq!(
        controller.on_commit_tick().expect("stable prefix").text,
        "before\n"
    );

    controller.push_delta("\n", now);

    let rendered = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("stable html block")
        .text;
    assert!(rendered.contains("<div>"));
    assert!(rendered.contains("content"));
}

#[test]
fn releases_raw_html_after_explicit_closing_tag() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n<script>\nconsole.log(1);\n</script>\n", now);

    let rendered = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("stable raw html")
        .text;
    assert!(rendered.contains("before"));
    assert!(rendered.contains("<script>"));
    assert!(rendered.contains("console.log(1);"));
}

#[test]
fn finalize_preserves_unclosed_fenced_code_source() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n```rust\nfn main() {}", now);

    assert_eq!(
        controller.finalize().cell.expect("final cell").text,
        "before\n```rust\nfn main() {}"
    );
}

#[test]
fn visible_source_keeps_emitted_prefix_while_tail_updates() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("### 标题\n", now);
    assert_eq!(
        controller.on_commit_tick().expect("stable cell").text,
        "### 标题\n"
    );

    controller.push_delta("正在生成", now);

    assert_eq!(controller.current_visible_text(), "### 标题\n正在生成");
}

#[test]
fn visible_source_holds_ambiguous_markdown_tail_without_losing_raw_source() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n**重点\n", now);

    assert_eq!(controller.current_visible_text(), "before\n");
    controller.set_width(32, now);
    assert_eq!(controller.current_visible_text(), "before\n");

    controller.push_delta("继续**\n", now);

    assert_eq!(
        controller.current_visible_text(),
        "before\n**重点\n继续**\n"
    );
    assert_eq!(
        controller.finalize().cell.expect("final cell").text,
        "before\n**重点\n继续**\n"
    );
}

#[test]
fn malformed_cjk_heading_waits_until_normalizable_line_boundary() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("##标题", now);
    assert!(controller.current_visible_text().is_empty());
    assert!(controller.current_tail_lines().is_empty());

    controller.push_delta("\n", now);
    assert_eq!(controller.current_visible_text(), "##标题\n");
}

#[test]
fn live_task_metadata_uses_separate_lines_before_stream_finishes() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("### 任务标题\u{3000}-\u{00a0}状态：进行中", now);
    let status_only = controller.current_visible_text();
    assert!(
        status_only
            .lines()
            .any(|line| line.contains("状态：进行中")),
        "流式状态应立即出现在独立视觉行：\n{status_only}"
    );
    assert!(!status_only.contains('•'));

    controller.push_delta(" - 任务目标：核验配置并给出修复建议", now);
    let completed = controller.current_visible_text();
    assert!(
        completed.lines().any(|line| line.contains("状态：进行中"))
            && completed.lines().any(|line| line.contains("任务目标：")),
        "状态与任务目标应分别渲染：\n{completed}"
    );
    assert!(!completed.contains('•'));
}

#[test]
fn resize_rebuilds_queue_from_emitted_boundary() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");
    controller.push_delta("a\nb\n", now);

    assert!(controller.on_commit_tick().is_some());
    controller.set_width(40, now);

    assert_eq!(controller.queued_lines(), 1);
    assert_eq!(
        controller.on_commit_tick().expect("stable cell").text,
        "a\nb\n"
    );
}

#[test]
fn unchanged_width_keeps_streaming_layout_cache() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");
    controller.push_delta("稳定的流式 Markdown 内容\n", now);

    let before = controller.current_active_hyperlink_lines();
    controller.set_width(80, now);
    let after = controller.current_active_hyperlink_lines();

    assert!(Arc::ptr_eq(&before, &after));
}

#[test]
fn stable_prefix_cache_invalidates_after_source_growth() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n- first\n- second\n", now);
    let first = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("first stable prefix")
        .text;
    assert!(first.contains("first"));
    assert!(!first.contains("second"));

    controller.push_delta("\n", now);
    let second = controller
        .on_commit_tick_batch(usize::MAX)
        .expect("released cached tail after source growth")
        .text;
    assert!(second.contains("second"));
}

#[test]
fn incremental_plain_text_layout_matches_canonical_markdown_render() {
    const STREAM_WIDTH: u16 = 80;
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", STREAM_WIDTH, ".");
    let mut source = String::new();

    for index in 0..1_000 {
        let delta = format!("普通响应第 {index} 行\n");
        source.push_str(&delta);
        controller.push_delta(&delta, now);
    }

    let actual = line_texts(&visible_lines(&controller.current_active_hyperlink_lines()));
    let expected = line_texts(&render_streaming_markdown_lines(&source, STREAM_WIDTH));

    assert_eq!(actual, expected);
}

#[test]
fn active_layout_matches_canonical_render_for_each_streaming_revision() {
    const NARROW_WIDTH: u16 = 24;
    const WIDE_WIDTH: u16 = 80;
    let scenarios = [
        "第一段普通文本足够长，需要在窄窗口中换行展示。\n第二段继续保持普通文本。\n",
        "普通前缀\n**未闭合强调\n已闭合**\n",
        "普通前缀\n| 名称 | 状态 |\n| --- | --- |\n| 流式表格 | 保持结构 |\n\n",
        "| 名称 | 状态 |\n| --- | --- |\n| 流式表格 | 保持结构 |\n\n",
        "```rust\nlet value = 1;\n```\n",
        "- 第一项\n- 第二项\n\n",
        "<div>\n结构化内容\n</div>\n\n",
    ];

    for width in [NARROW_WIDTH, WIDE_WIDTH] {
        for source in scenarios {
            let now = Instant::now();
            let mut controller = AssistantStreamController::new("assistant:test", width, ".");

            for delta in source.split_inclusive('\n') {
                controller.push_delta(delta, now);
                let active_source = controller.current_active_source();
                let actual =
                    line_texts(&visible_lines(&controller.current_active_hyperlink_lines()));
                let expected = line_texts(&render_streaming_markdown_lines(&active_source, width));

                assert_eq!(
                    actual, expected,
                    "活动布局偏离 canonical Markdown：width={width}, delta={delta:?}, source:\n{active_source}"
                );
            }
        }
    }
}

#[test]
fn active_source_rerenders_table_split_across_deltas() {
    const STREAM_WIDTH: u16 = 80;
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", STREAM_WIDTH, ".");

    controller.push_delta("| 现状 | 状态 |\n", now);
    assert!(
        controller
            .current_active_source()
            .contains("| 现状 | 状态 |")
    );

    controller.push_delta("| --- | --- |\n", now);
    controller.push_delta("| 流式展示 | 正常 |\n", now);

    let rendered = line_texts(&render_streaming_markdown_lines(
        &controller.current_active_source(),
        STREAM_WIDTH,
    ))
    .join("\n");

    assert!(rendered.contains('┌') && rendered.contains('┘'));
    assert!(rendered.contains("流式展示"));
}

#[test]
fn active_source_keeps_unstable_non_table_markdown_hidden() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("before\n**重点\n", now);

    assert_eq!(controller.current_active_source(), "before\n");
}

#[test]
fn active_source_does_not_show_partial_table_row() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta("| A | B |\n| --- | --- |\n| 1 | 2 |\n", now);
    controller.push_delta("| partial", now);

    assert!(!controller.current_active_source().contains("partial"));

    controller.push_delta("\n", now);
    assert!(controller.current_active_source().contains("partial"));
}

#[test]
fn resize_keeps_active_table_source_and_reflows_grid() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");

    controller.push_delta(
        "| Header | Value |\n| --- | --- |\n| A long value | remains visible |\n",
        now,
    );
    controller.set_width(24, now);

    assert_eq!(controller.queued_lines(), 0);
    let source = controller.current_active_source();
    assert!(source.contains("| Header | Value |"));
    let rendered = line_texts(&render_streaming_markdown_lines(&source, 24)).join("\n");
    assert!(rendered.contains('┌') && rendered.contains("remains"));
}

#[test]
fn queued_line_drain_does_not_reproject_unchanged_source() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");
    controller.push_delta("1\n2\n3\n", now);

    assert_eq!(
        controller.on_commit_tick().expect("first stable cell").text,
        "1\n2\n3\n"
    );
    assert!(
        controller.on_commit_tick_batch(2).is_none(),
        "同一 source 的剩余队列只能排空，不能重新投影活动 cell"
    );
    assert_eq!(controller.queued_lines(), 0);

    controller.push_delta("4\n", now);
    assert_eq!(
        controller.on_commit_tick().expect("grown source cell").text,
        "1\n2\n3\n4\n"
    );
}

#[test]
fn long_stream_evicts_animation_backlog_without_losing_active_markdown_source() {
    let now = Instant::now();
    let mut controller = AssistantStreamController::new("assistant:test", 80, ".");
    let line_count = STREAM_QUEUED_LINE_CAPACITY.saturating_add(32);
    let source = (0..line_count)
        .map(|index| format!("line-{index}\n"))
        .collect::<String>();

    controller.push_delta(&source, now);

    assert_eq!(controller.queued_lines(), STREAM_QUEUED_LINE_CAPACITY);
    assert_eq!(
        controller
            .take_active_projection_cell()
            .expect("active source should remain projectable")
            .text,
        source
    );
}
