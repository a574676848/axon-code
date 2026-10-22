use ratatui::style::Style;

use crate::{
    app_shell::{
        style_tokens::{MESSAGE_INITIAL_PREFIX, TerminalPaletteDepth},
        syntax_theme::{SyntaxTheme, SyntaxThemeName},
    },
    surface::display_width::DisplayWidthText,
    widgets::{
        terminal_hyperlinks::visible_lines,
        transcript_markdown::render_markdown_hyperlink_lines,
        transcript_markdown_test_support::{
            line_text, render_message_markdown, rendered_plain_lines, rendered_text,
        },
    },
};

#[test]
fn message_prefix_uses_marker_without_role_label() {
    let text = rendered_text("你好");

    assert_eq!(text, format!("{MESSAGE_INITIAL_PREFIX}你好"));
    assert!(!text.contains("用户"));
    assert!(!text.contains("助手"));
}

#[test]
fn multiline_plain_text_uses_marker_only_on_first_line() {
    let text = rendered_text("第一行\n第二行\n第三行");

    assert_eq!(text.matches(MESSAGE_INITIAL_PREFIX).count(), 1);
    assert!(text.contains("第一行"));
    assert!(text.contains("第二行"));
    assert!(text.contains("第三行"));
}

#[test]
fn wrapped_plain_text_uses_marker_only_on_first_visual_line() {
    let text = render_message_markdown("alpha beta gamma delta", Style::default(), 12)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(text.matches(MESSAGE_INITIAL_PREFIX).count(), 1);
    assert!(text.lines().count() > 1, "测试输入应触发自动换行：\n{text}");
}

#[test]
fn renders_markdown_lists_and_task_items_without_raw_markers() {
    let text = rendered_text("### 排查问题\n1. 分析报错来源\n- [x] 保持洋葱架构\n- [ ] 补齐验证");

    assert!(text.contains("排查问题"));
    assert!(text.contains("1. 分析报错来源\n"));
    assert!(text.contains("☑ 保持洋葱架构\n"));
    assert!(text.contains("☐ 补齐验证"));
    assert!(!text.contains("[x]"));
    assert!(!text.contains("[ ]"));
}

#[test]
fn headings_render_without_raw_hash_prefixes() {
    let text = rendered_text("# 顶层标题\n正文段落\n## 二级小节\n正文段落二");
    assert!(text.contains("顶层标题"));
    assert!(text.contains("二级小节"));
    assert!(!text.contains("# 顶层标题"));
    assert!(!text.contains("## 二级小节"));
}

#[test]
fn malformed_cjk_heading_is_normalized_before_rendering() {
    let text = rendered_text("##标题");

    assert!(text.contains("标题"));
    assert!(!text.contains("##标题"));
}

#[test]
fn escaped_and_code_hashes_remain_literal_content() {
    let text = rendered_text("\\## 字面量\n\n`## inline`\n\n```text\n## code\n```");

    assert!(text.contains("## 字面量"));
    assert!(text.contains("## inline"));
    assert!(text.contains("## code"));
}

#[test]
fn h3_keeps_rich_style_without_overwhelming_prefix() {
    // H3+ 在密集转录中不再额外加 ### 前缀，避免视觉噪音。
    let text = rendered_text("### 子节\n内容");
    assert!(text.contains("子节"));
    assert!(!text.contains("###"), "H3 不应输出 ### 前缀：\n{text}");
}

#[test]
fn blockquote_prefix_survives_soft_breaks_and_nesting() {
    let text = rendered_text("> 第一层\n> > 第二层\n> 延续行");

    assert!(text.contains("> 第一层"));
    assert!(text.contains("> > 第二层"));
    assert!(text.contains("> 延续行"));
}

#[test]
fn blockquote_and_list_prefixes_compose_without_raw_markdown_leakage() {
    let text = rendered_text("> - 引用列表项\n>   继续说明\n\n- 普通项\n  > 内部引用");

    assert!(text.contains("> - 引用列表项"));
    assert!(text.contains(">   继续说明"));
    assert!(text.contains("- 普通项"));
    assert!(text.contains("  > 内部引用"));
}

#[test]
fn blockquote_inside_list_item_stays_on_list_marker_line() {
    let rendered = rendered_plain_lines("1.\n   > quoted\n   after\n");

    assert_eq!(rendered, vec!["• 1. > quoted", "     > after"]);
}

#[test]
fn loose_list_item_paragraphs_keep_continuation_indent() {
    let rendered = rendered_plain_lines(
        "1. First paragraph\n\n   Second paragraph of same item\n\n2. Next item\n",
    );

    assert_eq!(
        rendered,
        vec![
            "• 1. First paragraph",
            "",
            "     Second paragraph of same item",
            "",
            "  2. Next item",
        ]
    );
}

#[test]
fn list_code_block_keeps_code_indent_and_separator_like_codex() {
    let rendered =
        rendered_plain_lines("1. First:\n\n   ```rust\n   fn first() {}\n   ```\n\n2. Second:\n");

    assert_eq!(
        rendered,
        vec!["• 1. First:", "", "     fn first() {}", "", "  2. Second:"]
    );
}

#[test]
fn nested_code_block_keeps_outer_sibling_separator_like_codex() {
    let rendered = rendered_plain_lines(
        "1. First:\n   - Nested:\n\n     ```rust\n     fn first() {}\n     ```\n\n2. Second:\n",
    );

    assert_eq!(
        rendered,
        vec![
            "• 1. First:",
            "      - Nested:",
            "",
            "        fn first() {}",
            "",
            "  2. Second:",
        ]
    );
}

#[test]
fn ordered_item_with_code_block_and_nested_bullet_matches_codex_structure() {
    let rendered = rendered_plain_lines(
        "1. **item 1**\n\n2. **item 2**\n   ```\n   code\n   ```\n   - `PROCESS_START` (a `OnceLock<Instant>`) keeps the start time.\n",
    );

    assert_eq!(
        rendered,
        vec![
            "• 1. item 1",
            "  2. item 2",
            "",
            "     code",
            "      - PROCESS_START (a OnceLock<Instant>) keeps the start time.",
        ]
    );
}

#[test]
fn nested_five_levels_mixed_lists_keep_codex_indent_ladder() {
    let rendered = rendered_plain_lines(
        "1. First\n   - Second level\n     1. Third level (ordered)\n        - Fourth level (bullet)\n          - Fifth level to test indent consistency\n",
    );

    assert_eq!(
        rendered,
        vec![
            "• 1. First",
            "      - Second level",
            "          1. Third level (ordered)",
            "              - Fourth level (bullet)",
            "                  - Fifth level to test indent consistency",
        ]
    );
}

#[test]
fn html_in_tight_ordered_item_keeps_list_continuation_indent() {
    let rendered = rendered_plain_lines("1. Foo\n   <i>Bar</i>\n");

    assert_eq!(rendered, vec!["• 1. Foo", "     <i>Bar</i>"]);
}

#[test]
fn html_continuation_paragraph_in_unordered_item_is_indented() {
    let rendered = rendered_plain_lines("- Item\n\n  <em>continued</em>\n");

    assert_eq!(rendered, vec!["• - Item", "", "    <em>continued</em>"]);
}

#[test]
fn nested_item_continuation_paragraph_keeps_deep_indent_and_sibling_separator() {
    let rendered = rendered_plain_lines("1. A\n    - B\n\n      Continuation for B\n2. C\n");

    assert_eq!(
        rendered,
        vec![
            "• 1. A",
            "      - B",
            "",
            "        Continuation for B",
            "",
            "  2. C",
        ]
    );
}

#[test]
fn crlf_code_block_does_not_insert_spurious_blank_lines() {
    let rendered = rendered_plain_lines("```rust\r\nfn main() {}\r\n    line2\r\n```\r\n");

    assert!(
        rendered.windows(2).any(|window| {
            window[0].contains("fn main() {}") && window[1].contains("    line2")
        }),
        "CRLF code block 不应在连续代码行之间插入空行：\n{rendered:#?}"
    );
}

#[test]
fn horizontal_rule_renders_visible_separator() {
    let text = rendered_text("Before\n\n---\n\nAfter");

    assert!(text.contains("Before"));
    assert!(text.contains("———"));
    assert!(text.contains("After"));
    assert!(!text.contains("---"));
}

#[test]
fn blockquote_horizontal_rule_keeps_quote_prefix() {
    let text = rendered_text("> Before\n>\n> ---\n>\n> After");

    assert!(text.contains("> Before"));
    assert!(text.contains("> ———"));
    assert!(text.contains("> After"));
}

#[test]
fn wrapped_list_item_keeps_continuation_indent() {
    let lines = render_message_markdown("- alpha beta gamma delta epsilon", Style::default(), 18)
        .into_iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>();

    assert!(lines.iter().any(|line| line.contains("- alpha")));
    assert!(
        lines
            .iter()
            .skip(1)
            .any(|line| line.contains("  beta") || line.contains("  gamma")),
        "换行后的列表内容应保持 continuation 缩进：\n{lines:#?}"
    );
}

#[test]
fn wrapped_list_item_is_separated_from_next_sibling_like_codex() {
    let lines = render_message_markdown(
        "1. This item wraps onto another visible rendered line\n2. Next item\n",
        Style::default(),
        24,
    )
    .iter()
    .map(line_text)
    .collect::<Vec<_>>();
    let next_item = lines
        .iter()
        .position(|line| line.contains("2. Next item"))
        .expect("下一个 ordered sibling 应被渲染");

    assert!(
        next_item > 0 && lines[next_item - 1].trim().is_empty(),
        "自动换行后的列表项与下一个 sibling 之间应保留 Codex 风格分隔：\n{lines:#?}"
    );
}

#[test]
fn block_html_keeps_following_paragraph_on_next_line() {
    let text = rendered_text("<div>状态</div>\n后续说明");

    assert!(
        text.contains("<div>状态</div>\n"),
        "块级 HTML 后应自然断行：\n{text}"
    );
    assert!(text.contains("后续说明"));
    assert!(
        !text.contains("<div>状态</div>后续说明"),
        "块级 HTML 不应和后续段落粘连：\n{text}"
    );
}

#[test]
fn inline_html_stays_in_current_sentence() {
    let text = rendered_text("状态 <span>ok</span> 完成");

    assert!(text.contains("状态 <span>ok</span> 完成"));
}

#[test]
fn fenced_code_uses_first_info_token_for_highlighting_without_visible_label() {
    let theme = SyntaxTheme::new_with_depth(
        SyntaxThemeName::CatppuccinMocha,
        TerminalPaletteDepth::TrueColor,
    );
    let lines = render_markdown_hyperlink_lines(
        "```rust ignore\nlet x = 42;\n```",
        Style::default(),
        120,
        Some(&theme),
    );
    let code_line = visible_lines(&lines)
        .into_iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("let")))
        .expect("code line rendered");
    let text = line_text(&code_line);

    assert!(!text.contains("`rust ignore`"));
    assert!(!text.contains("`rust`"));
    assert!(text.contains("let x = 42;"));
    assert!(
        code_line
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .collect::<std::collections::HashSet<_>>()
            .len()
            >= 2,
        "应使用 rust 语法高亮输出 token span: {code_line:?}"
    );
}

#[test]
fn emits_hyperlink_metadata_for_web_and_file_links() {
    let lines = render_markdown_hyperlink_lines(
        "查看 [官网](https://example.com/docs) 和 [源码](crates/axon-tui-runtime/src/lib.rs)",
        Style::default(),
        120,
        None,
    );
    let destinations = lines
        .iter()
        .flat_map(|line| line.hyperlinks.iter())
        .map(|link| link.destination.as_str())
        .collect::<Vec<_>>();

    assert!(destinations.contains(&"https://example.com/docs"));
    assert!(
        destinations
            .iter()
            .any(|destination| destination.starts_with("file://"))
    );
}

#[test]
fn web_markdown_link_renders_visible_destination() {
    let text = rendered_text("查看 [官网](https://example.com/docs)");

    assert!(text.contains("官网 (https://example.com/docs)"));
}

#[test]
fn non_file_uri_markdown_link_keeps_label_and_visible_destination() {
    let text = rendered_text("联系 [mail](mailto:test@example.com) 或 [urn](urn:example:resource)");

    assert!(text.contains("mail (mailto:test@example.com)"));
    assert!(text.contains("urn (urn:example:resource)"));
    assert!(!text.contains("file://"));
}

#[test]
fn reference_link_and_parenthesized_url_render_like_codex() {
    let text = rendered_text(
        "Auto-link: <https://example.com> and reference link [ref][r1].\n\
URL with parentheses: [link](https://example.com/path_(with)_parens).\n\n\
[r1]: https://example.com/ref \"Reference link title\"",
    );

    assert!(text.contains("https://example.com (https://example.com)"));
    assert!(text.contains("ref (https://example.com/ref)"));
    assert!(text.contains("link (https://example.com/path_(with)_parens)"));
}

#[test]
fn local_file_markdown_link_uses_target_path_as_visible_text() {
    let text =
        rendered_text("查看 [源码](crates/axon-tui-runtime/src/widgets/transcript_markdown.rs:74)");

    assert!(
        text.contains("crates/axon-tui-runtime/src/widgets/transcript_markdown.rs:74"),
        "本地链接应显示目标路径和行号：\n{text}"
    );
    assert!(
        !text.contains("源码"),
        "本地链接不应只显示泛化 label：\n{text}"
    );
}

#[test]
fn local_file_markdown_link_preserves_range_location_suffix() {
    let lines = render_markdown_hyperlink_lines(
        "查看 [源码](crates/axon-tui-runtime/src/widgets/transcript_markdown.rs:74:3-80:4)",
        Style::default(),
        120,
        None,
    );
    let text = visible_lines(&lines)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>()
        .join("\n");
    let destinations = lines
        .iter()
        .flat_map(|line| line.hyperlinks.iter())
        .map(|link| link.destination.as_str())
        .collect::<Vec<_>>();

    assert!(
        text.contains("crates/axon-tui-runtime/src/widgets/transcript_markdown.rs:74:3-80:4"),
        "本地链接 range 应按 Codex 风格显示：\n{text}"
    );
    assert!(
        destinations
            .iter()
            .any(|destination| destination.ends_with("transcript_markdown.rs:74:3-80:4")),
        "hyperlink destination 应保留 range 后缀：\n{destinations:#?}"
    );
}

#[test]
fn list_local_file_link_colon_continuation_stays_inline() {
    let lines = render_message_markdown(
        "- [runtime](crates/axon-tui-runtime/src/lib.rs:1)\n  : TUI runtime 入口",
        Style::default(),
        120,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .any(|line| line.contains("- crates/axon-tui-runtime/src/lib.rs:1: TUI runtime 入口")),
        "本地链接后的冒号说明应保持在同一列表项：\n{rendered:#?}"
    );
}

#[test]
fn fenced_rust_block_emits_highlighted_spans_not_single_code_span() {
    let theme = SyntaxTheme::new_with_depth(
        SyntaxThemeName::CatppuccinMocha,
        TerminalPaletteDepth::TrueColor,
    );
    let lines = render_markdown_hyperlink_lines(
        "```rust\nlet x = 42;\n```",
        Style::default(),
        120,
        Some(&theme),
    );
    let code_line = visible_lines(&lines)
        .into_iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("let")))
        .expect("code line rendered");
    let text = line_text(&code_line);

    assert!(text.contains("let x = 42;"));
    assert!(
        code_line.spans.len() >= 2,
        "应输出 token 级 span: {code_line:?}"
    );
    assert!(
        code_line
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .map(|color| format!("{color:?}"))
            .collect::<std::collections::HashSet<_>>()
            .len()
            >= 2,
        "应至少包含两类语义色: {code_line:?}"
    );
}

#[test]
fn fenced_unknown_language_emits_single_fallback_span() {
    let theme = SyntaxTheme::new(SyntaxThemeName::CatppuccinMocha);
    let lines = render_markdown_hyperlink_lines(
        "```nosuchlang\nfoo\n```",
        Style::default(),
        120,
        Some(&theme),
    );
    let code_line = visible_lines(&lines)
        .into_iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("foo")))
        .expect("code line rendered");

    let code_spans = code_line
        .spans
        .iter()
        .filter(|span| !span.content.trim().is_empty())
        .collect::<Vec<_>>();

    assert_eq!(code_spans.len(), 1);
    assert_eq!(code_spans[0].content.as_ref(), "foo");
}

#[test]
fn fenced_block_without_theme_keeps_single_fallback_span() {
    let lines =
        render_markdown_hyperlink_lines("```rust\nlet x = 42;\n```", Style::default(), 120, None);
    let code_line = visible_lines(&lines)
        .into_iter()
        .find(|line| line.spans.iter().any(|span| span.content.contains("let")))
        .expect("code line rendered");

    let code_spans = code_line
        .spans
        .iter()
        .filter(|span| !span.content.trim().is_empty())
        .collect::<Vec<_>>();

    assert_eq!(code_spans.len(), 1);
    assert_eq!(code_spans[0].content.as_ref(), "let x = 42;");
}

#[test]
fn fenced_code_preserves_blank_line_inside_block() {
    let lines = render_message_markdown(
        "```rust\nfn main() {}\n\nprintln!(\"done\");\n```",
        Style::default(),
        120,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();
    let first_code_index = rendered
        .iter()
        .position(|line| line.contains("fn main() {}"))
        .expect("first code line rendered");

    assert!(
        rendered
            .get(first_code_index + 1)
            .is_some_and(|line| line.trim().is_empty()),
        "代码块内部空行不应丢失：\n{rendered:#?}"
    );
    assert!(
        rendered
            .get(first_code_index + 2)
            .is_some_and(|line| line.contains("println!(\"done\");")),
        "代码块内部空行后的代码应保持顺序：\n{rendered:#?}"
    );
}

#[test]
fn indented_code_block_preserves_commonmark_visual_indent() {
    let rendered = rendered_text("Before\n\n    fn main() {}\n      println!(\"hi\");\n\nAfter");

    assert!(rendered.contains("Before"));
    assert!(
        rendered.contains("    fn main() {}"),
        "缩进代码块首行应保留四空格视觉前缀：\n{rendered}"
    );
    assert!(
        rendered.contains("      println!(\"hi\");"),
        "缩进代码块内部额外缩进应保留：\n{rendered}"
    );
    assert!(rendered.contains("After"));
}

#[test]
fn top_level_paragraphs_keep_a_readable_blank_line() {
    let rendered = rendered_plain_lines("第一段内容。\n\n第二段内容。");

    assert_eq!(rendered, vec!["• 第一段内容。", "", "  第二段内容。"],);
}

#[test]
fn headings_and_following_paragraphs_keep_visual_separation() {
    let rendered = rendered_plain_lines("# 章节标题\n\n正文内容。");

    assert_eq!(rendered, vec!["• 章节标题", "", "  正文内容。"],);
}

#[test]
fn dense_task_heading_splits_metadata_and_keeps_wrapped_objective_indented() {
    let lines = render_message_markdown(
        "### [AXON-MCP-001]: 定位 Morph MCP 工具加载失败原因 - 状态:进行中 - 任务目标: 核验当前 MCP 配置、启动链路与日志，给出可复现的根因和修复建议。",
        Style::default(),
        52,
    )
    .iter()
    .map(line_text)
    .collect::<Vec<_>>();

    assert!(
        lines
            .iter()
            .all(|line| !(line.contains("状态") && line.contains("任务目标"))),
        "状态与任务目标不应挤在同一视觉行：\n{lines:#?}",
    );
    let objective_line = lines
        .iter()
        .position(|line| line.contains("任务目标"))
        .expect("任务目标应独立成行");
    let objective_indent = lines[objective_line]
        .chars()
        .take_while(|ch| ch.is_whitespace())
        .count();
    assert!(
        lines.get(objective_line + 1).is_some_and(|line| line
            .chars()
            .take_while(|ch| ch.is_whitespace())
            .count()
            > objective_indent),
        "任务目标自动换行后应保持列表悬挂缩进：\n{lines:#?}",
    );
}

#[test]
fn nested_list_item_with_url_wraps_without_visible_overflow() {
    const WIDTH: u16 = 32;
    const SOURCE: &str = "- 外层项目\n  - 嵌套项目使用默认 base URL (https://example.com/api/v1/models/very-long-default-base-url) 并在恢复后保留完整结果";
    let rendered = render_message_markdown(SOURCE, Style::default(), WIDTH)
        .iter()
        .map(line_text)
        .collect::<Vec<_>>();

    assert!(
        rendered
            .iter()
            .all(|line| DisplayWidthText::width(line) <= usize::from(WIDTH)),
        "嵌套列表含 URL 时每个可见行都必须处于宽度预算内：\n{rendered:#?}"
    );
    let actual = rendered
        .join("")
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '•')
        .collect::<String>();
    let expected = SOURCE
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();

    assert_eq!(
        actual, expected,
        "换行不得丢失嵌套列表正文：\n{rendered:#?}"
    );
}
