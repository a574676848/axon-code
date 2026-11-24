use ratatui::style::Style;

use crate::widgets::{
    terminal_hyperlinks::visible_lines,
    transcript_cells::display_width,
    transcript_markdown::render_markdown_hyperlink_lines,
    transcript_markdown_test_support::{
        line_text, render_message_markdown, rendered_plain_lines, rendered_text,
    },
};

#[test]
fn unwraps_markdown_table_fence_before_rendering() {
    let text =
        rendered_text("```markdown\n| 文件 | 状态 |\n| --- | --- |\n| src/main.rs | 已修改 |\n```");

    assert!(text.contains("文件"));
    assert!(text.contains("状态"));
    assert!(text.contains("━"), "表头分隔应使用重线 ━");
    assert!(text.contains("src/main.rs"));
    assert!(!text.contains("```markdown"));
    assert!(!text.contains("| --- | --- |"));
}

#[test]
fn renders_no_outer_pipe_table_as_structured_table() {
    let text = rendered_text("文件 | 状态\n--- | ---\nsrc/main.rs | 已修改");

    assert!(text.contains("文件"));
    assert!(text.contains("状态"));
    assert!(text.contains("━"), "无外侧 pipe 表格应结构化渲染：\n{text}");
    assert!(!text.contains("--- | ---"));
}

#[test]
fn escaped_pipe_stays_inside_table_cell() {
    let text = rendered_text("名称 | 说明\n--- | ---\nA \\| B | 保留在同一格");

    assert!(text.contains("A | B"), "escaped pipe 不应拆列：\n{text}");
    assert!(text.contains("保留在同一格"));
    assert!(!text.contains("--- | ---"));
}

#[test]
fn table_alignment_respects_commonmark_markers() {
    let lines = rendered_plain_lines(
        "| 左 | 中 | 右 |\n| :--- | :---: | ---: |\n| a | b | c |\n| long | mid | tail |",
    );
    let short_row = lines
        .iter()
        .find(|line| line.contains('a') && line.contains('b') && line.contains('c'))
        .expect("短数据行应被渲染");
    let long_row = lines
        .iter()
        .find(|line| line.contains("long") && line.contains("mid") && line.contains("tail"))
        .expect("长数据行应被渲染");

    assert!(
        short_row.contains("│ a") && short_row.contains("│  b") && short_row.contains("│    c │"),
        "短数据行应按左/中/右对齐标记填充：\n{lines:#?}"
    );
    assert!(short_row.find('b') > long_row.find("mid"));
    assert!(short_row.find('c') > long_row.find("tail"));
}

#[test]
fn table_cells_preserve_hyperlink_metadata() {
    let lines = render_markdown_hyperlink_lines(
        "| 名称 | 链接 |\n| --- | --- |\n| docs | [官网](https://example.com/docs) |",
        Style::default(),
        118,
        None,
    );
    let destinations = lines
        .iter()
        .flat_map(|line| line.hyperlinks.iter())
        .map(|link| link.destination.as_str())
        .collect::<Vec<_>>();

    assert!(destinations.contains(&"https://example.com/docs"));
}

#[test]
fn renders_table_after_intro_paragraph_as_structured_table() {
    let text = rendered_text(
        "建议优先排这些任务：\n| 优先级 | 优化任务 | 用户价值 |\n| --- | --- | --- |\n| P0 | 配置示例改成真实 TOML | 首次 setup 少踩坑 |\n| P1 | 上下文压缩报告 | 长会话不丢关键需求 |",
    );

    assert!(text.contains("建议优先排这些任务"));
    assert!(text.contains("优先级"));
    assert!(text.contains("优化任务"));
    assert!(text.contains("━"), "表格应进入结构化渲染：\n{text}");
    assert!(
        !text.contains("| ---"),
        "不应泄露 raw markdown 分隔行：\n{text}"
    );
}

#[test]
fn paragraph_after_table_stays_outside_structured_table() {
    let text = rendered_text(
        "| 名称 | 状态 |\n| --- | --- |\n| README | 已更新 |\n后续说明应作为普通段落显示",
    );

    assert!(text.contains("README"));
    assert!(text.contains("已更新"));
    assert!(text.contains("后续说明应作为普通段落显示"));
    assert!(
        !text.contains("后续说明应作为普通段落显示  "),
        "普通段落不应被补齐为空表格列：\n{text}"
    );
}

#[test]
fn link_after_table_keeps_hyperlink_metadata() {
    let lines = render_markdown_hyperlink_lines(
        "| 名称 | 状态 |\n| --- | --- |\n| README | 已更新 |\n查看 [官网](https://example.com/docs)",
        Style::default(),
        118,
        None,
    );
    let destinations = lines
        .iter()
        .flat_map(|line| line.hyperlinks.iter())
        .map(|link| link.destination.as_str())
        .collect::<Vec<_>>();

    assert!(destinations.contains(&"https://example.com/docs"));
}

#[test]
fn table_uses_header_heavy_and_body_thin_separators() {
    let lines = render_message_markdown(
        "| col1 | col2 |\n| --- | --- |\n| a | b |\n| c | d |\n| e | f |",
        Style::default(),
        80,
    );
    let separator_lines = lines
        .iter()
        .filter(|line| {
            line.spans
                .iter()
                .any(|span| span.content.contains('━') || span.content.contains('─'))
        })
        .collect::<Vec<_>>();
    let heavy_count = separator_lines
        .iter()
        .filter(|line| line.spans.iter().any(|span| span.content.contains('━')))
        .count();
    let thin_count = separator_lines
        .iter()
        .filter(|line| line.spans.iter().any(|span| span.content.contains('─')))
        .count();

    assert_eq!(heavy_count, 2, "顶边与表头边界都应使用重线");
    assert_eq!(thin_count, 3, "数据边界与底边应使用细线");
}

#[test]
fn narrow_markdown_table_keeps_complete_grid_and_wraps_cells() {
    let text = render_message_markdown(
        "| 文件 | 说明 |\n| --- | --- |\n| crates/axon-tui-runtime/src/widgets/transcript_markdown.rs | 修复表格在窄屏下的 raw markdown 和错位问题 |\n| crates/axon-tui-runtime/src/widgets/command_palette.rs | 对齐长模型名称候选 |",
        Style::default(),
        42,
    )
    .iter()
    .map(line_text)
    .collect::<Vec<_>>()
    .join("\n");

    assert!(
        text.contains('┌') && text.contains('└'),
        "实际渲染：\n{text}"
    );
    assert!(text.contains("axon-tui-runtim"), "实际渲染：\n{text}");
    assert!(text.contains('│'));
    assert!(!text.contains("| ---"));
}

#[test]
fn medium_width_markdown_table_keeps_grid_and_wraps_long_cells() {
    let rendered = render_message_markdown(
        "| 文件 | 状态 | 数量 |\n| --- | --- | ---: |\n| crates/axon-tui-runtime/src/widgets/transcript_markdown_table.rs | 已完成 | 12 |",
        Style::default(),
        72,
    )
    .iter()
    .map(line_text)
    .collect::<Vec<_>>();
    let text = rendered.join("\n");

    assert!(text.contains('━'), "中等宽度应保留表格网格：\n{text}");
    assert!(
        text.contains("transcript_markdown_table.rs"),
        "长路径应在 grid cell 内换行展示：\n{text}"
    );
    assert!(
        !text.contains("文件:"),
        "可读 grid 不应过早降级成记录列表：\n{text}"
    );
    assert!(
        rendered.iter().all(|line| display_width(line) <= 72),
        "表格行不应横向溢出：\n{rendered:#?}"
    );
}

#[test]
fn very_narrow_markdown_table_still_renders_complete_grid() {
    let text = render_message_markdown(
        "| 文件 | 状态 | 数量 |\n| --- | --- | ---: |\n| crates/axon-tui-runtime/src/widgets/transcript_markdown_table.rs | 已完成 | 12 |",
        Style::default(),
        28,
    )
    .iter()
    .map(line_text)
    .collect::<Vec<_>>()
    .join("\n");

    assert!(
        text.contains('┌') && text.contains('┘'),
        "实际渲染：\n{text}"
    );
    assert!(text.contains("crat"), "实际渲染：\n{text}");
    assert!(text.contains("│ 已完成"));
    assert!(text.contains("│   12"));
    assert!(!text.contains("| ---"));
}

#[test]
fn narrow_grid_preserves_rich_values_and_hyperlinks() {
    let lines = render_markdown_hyperlink_lines(
        "| Key | Content | Extra | More |\n|---|---|---|---|\n| item | [link](https://example.com) | **bold** | `code` |\n",
        Style::default(),
        14,
        None,
    );
    let visible = visible_lines(&lines);
    let rendered = visible.iter().map(line_text).collect::<Vec<_>>();
    let destinations = lines
        .iter()
        .flat_map(|line| line.hyperlinks.iter())
        .map(|link| link.destination.as_str())
        .collect::<Vec<_>>();

    assert!(
        rendered.iter().any(|line| line.contains('┌')),
        "窄表格必须保留完整外框：\n{rendered:#?}"
    );
    assert!(rendered.iter().any(|line| line.contains("link")));
    assert!(rendered.iter().any(|line| line.contains("bol")));
    assert!(rendered.iter().any(|line| line.contains("cod")));
    assert!(destinations.contains(&"https://example.com"));
    assert!(
        visible.iter().any(|line| {
            line.spans.iter().any(|span| {
                span.style
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD)
                    && !span.content.trim().is_empty()
            })
        }),
        "grid value 中的 bold span 应保留样式：\n{visible:#?}"
    );
    assert!(
        visible.iter().any(|line| {
            line.spans
                .iter()
                .any(|span| span.style != Style::default() && !span.content.trim().is_empty())
        }),
        "grid value 中的 code span 应保留样式：\n{visible:#?}"
    );
}

#[test]
fn table_url_with_tilde_keeps_complete_hyperlink() {
    let destination = "https://www.cs.tufts.edu/~nr/cs257/archive/olin-shivers/dissertation.pdf";
    let markdown = format!("| URL |\n| --- |\n| {destination} |\n");
    let lines = render_markdown_hyperlink_lines(&markdown, Style::default(), 30, None);
    let rendered = visible_lines(&lines)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let destinations = lines
        .iter()
        .flat_map(|line| line.hyperlinks.iter())
        .map(|link| link.destination.as_str())
        .collect::<Vec<_>>();

    assert!(rendered.contains("cs.tufts.edu"), "实际渲染：\n{rendered}");
    assert!(!destinations.is_empty());
    assert_eq!(destinations, vec![destination; destinations.len()]);
}

#[test]
fn table_inside_blockquote_keeps_quote_prefix_on_all_lines() {
    let rendered = rendered_plain_lines("> | A | B |\n> |---|---|\n> | 1 | 2 |\n");

    assert!(
        rendered
            .iter()
            .all(|line| line.starts_with("• > ") || line.starts_with("  > ")),
        "引用内表格每一行都应保留 blockquote 前缀：\n{rendered:#?}"
    );
    assert!(
        rendered.iter().any(|line| line.contains("━━━━━")),
        "引用内表格仍应结构化渲染：\n{rendered:#?}"
    );
}

#[test]
fn renders_long_table_with_inline_code_and_escaped_pipes() {
    const TABLE_WIDTH: u16 = 96;
    let rendered = render_message_markdown(
        concat!(
            "| 类型 | 命令 | 说明 |\n",
            "| --- | --- | --- |\n",
            "| 会话恢复 | `axon resume` | 恢复已有会话并继续执行 |\n",
            "| MCP 管理 | `axon mcp` | 支持 `list\\|get\\|add\\|remove` 子命令 |",
        ),
        Style::default(),
        TABLE_WIDTH,
    );
    let text = rendered
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.contains('┌') && text.contains('└'),
        "长表格应保持结构化网格：\n{text}"
    );
    assert!(text.contains("axon resume"));
    for subcommand in ["list", "get", "add", "remove"] {
        assert!(
            text.contains(subcommand),
            "缺少子命令 {subcommand}：\n{text}"
        );
    }
    assert!(
        !text.contains("\\|"),
        "escaped pipe 不应泄露反斜杠：\n{text}"
    );
    assert!(!text.contains("| --- | --- |"));
    assert!(
        rendered
            .iter()
            .all(|line| line.width() <= usize::from(TABLE_WIDTH)),
        "表格行不应横向溢出：\n{rendered:#?}"
    );
}
