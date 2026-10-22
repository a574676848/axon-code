use ratatui::style::{Color, Modifier, Style};

use crate::{
    app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
    widgets::{
        transcript_markdown::render_markdown_hyperlink_lines,
        transcript_markdown_test_support::{
            line_text, render_message_markdown, rendered_plain_lines,
        },
    },
};

#[test]
fn blockquote_heading_keeps_quote_prefix_on_heading_and_paragraph() {
    let rendered = rendered_plain_lines("> # Heading\n> paragraph text\n");

    assert_eq!(
        rendered,
        vec!["• > Heading", "  > paragraph text"],
        "blockquote 内 heading 与后续段落都应走统一块前缀状态：\n{rendered:#?}"
    );
}

#[test]
fn blockquote_fenced_code_keeps_quote_prefix_on_each_code_line() {
    let rendered = rendered_plain_lines("> ```\n> first\n> second\n> ```\n");

    assert_eq!(
        rendered,
        vec!["• > first", "  > second"],
        "blockquote 内 fenced code 每一行都应保留引用前缀：\n{rendered:#?}"
    );
}

#[test]
fn nested_blockquote_with_inline_and_fenced_code_matches_codex_shape() {
    let rendered = rendered_plain_lines(
        r#"> Nested quote with code:
> > Inner quote and `inline code`
> >
> > ```
> > # fenced code inside a quote
> > echo "hello from a quote"
> > ```
"#,
    );

    assert_eq!(
        rendered,
        vec![
            "• > Nested quote with code:",
            "  >",
            "  > > Inner quote and inline code",
            "  > >",
            "  > > # fenced code inside a quote",
            "  > > echo \"hello from a quote\"",
        ],
        "嵌套 blockquote、inline code 与 fenced code 应共享同一前缀栈：\n{rendered:#?}"
    );
}

#[test]
fn consecutive_list_local_file_link_colon_continuations_stay_attached() {
    let lines = render_message_markdown(
        "- [runtime](crates/axon-tui-runtime/src/lib.rs:1)\n  : TUI runtime 入口\n- [widgets](crates/axon-tui-runtime/src/widgets/mod.rs:1)\n  : transcript widgets 入口",
        Style::default(),
        120,
    );
    let rendered = lines.iter().map(line_text).collect::<Vec<_>>();

    assert_eq!(
        rendered,
        vec![
            "• - crates/axon-tui-runtime/src/lib.rs:1: TUI runtime 入口",
            "  - crates/axon-tui-runtime/src/widgets/mod.rs:1: transcript widgets 入口",
        ],
        "连续本地文件链接的冒号说明不能从对应路径上脱落：\n{rendered:#?}"
    );
}

#[test]
fn styled_prefix_multiline_local_file_link_label_uses_target_path() {
    let rendered = rendered_plain_lines(
        "**bold** plain [foo\nbar](crates/axon-tui-runtime/src/widgets/transcript_markdown.rs:74:3)",
    );

    assert_eq!(
        rendered,
        vec!["• bold plain crates/axon-tui-runtime/src/widgets/transcript_markdown.rs:74:3"],
        "styled 前缀后的多行本地链接 label 应被目标路径替代且不拆行：\n{rendered:#?}"
    );
}

#[test]
fn persisted_response_keeps_body_list_and_strong_section_on_separate_lines() {
    let rendered = rendered_plain_lines(concat!(
        "已完成范式核验，未修改任何文件。  \n",
        "- `ova_mcp` 已检索业务对象。  \n",
        "- 下一步生成仅含 `--dry-run` 的发布脚本。  \n",
        "🧠 **Axon Memory 治理日志**：未沉淀记忆。",
    ));
    let body = rendered
        .iter()
        .position(|line| line.contains("已完成范式核验"))
        .expect("正文应可见");
    let first_item = rendered
        .iter()
        .position(|line| line.contains("ova_mcp"))
        .expect("首个列表项应可见");
    let second_item = rendered
        .iter()
        .position(|line| line.contains("--dry-run"))
        .expect("第二个列表项应可见");
    let governance = rendered
        .iter()
        .position(|line| line.contains("Axon Memory 治理日志"))
        .expect("治理标题应可见");

    assert!(
        body < first_item && first_item < second_item && second_item < governance,
        "正文、列表项和治理标题必须分别占据后续可见行：\n{rendered:#?}"
    );
}

#[test]
fn strong_inherits_body_and_inline_code_keeps_highlight_without_bold() {
    let lines = render_message_markdown("**重点结论**与 `--dry-run` 参数", Style::default(), 80);

    let strong = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "重点结论")
        .expect("粗体内容应保持独立样式 span");
    assert_eq!(strong.style.fg, None);
    assert!(strong.style.add_modifier.contains(Modifier::BOLD));

    let inline_code = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "--dry-run")
        .expect("行内代码应保持独立样式 span");
    assert_eq!(
        inline_code.style.fg,
        Some(transcript_palette().color(TranscriptColorRole::InlineHighlight))
    );
    assert!(!inline_code.style.add_modifier.contains(Modifier::BOLD));
}

#[test]
fn links_use_cyan_underlined_and_visible_url_keeps_link_style() {
    let lines = render_message_markdown(
        "查看 [官网](https://example.com/docs)",
        Style::default(),
        120,
    );
    let label = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "官网")
        .expect("链接文字应保持独立样式 span");
    assert_eq!(label.style.fg, Some(Color::Cyan));
    assert!(label.style.add_modifier.contains(Modifier::UNDERLINED));

    let destination = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "https://example.com/docs")
        .expect("可见链接目标应保持独立样式 span");
    assert_eq!(destination.style.fg, Some(Color::Cyan));
    assert!(
        destination
            .style
            .add_modifier
            .contains(Modifier::UNDERLINED)
    );
}

#[test]
fn headings_use_codex_modifier_hierarchy_without_fixed_foreground() {
    let lines = render_message_markdown(
        "# 一级\n\n## 二级\n\n### 三级\n\n#### 四级",
        Style::default(),
        120,
    );
    let find = |content: &str| {
        lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .find(|span| span.content.as_ref() == content)
            .unwrap_or_else(|| panic!("未找到标题 span: {content}"))
    };

    let h1 = find("一级");
    assert_eq!(h1.style.fg, None);
    assert!(h1.style.add_modifier.contains(Modifier::BOLD));
    assert!(h1.style.add_modifier.contains(Modifier::UNDERLINED));

    let h2 = find("二级");
    assert_eq!(h2.style.fg, None);
    assert!(h2.style.add_modifier.contains(Modifier::BOLD));
    assert!(!h2.style.add_modifier.contains(Modifier::UNDERLINED));

    let h3 = find("三级");
    assert!(h3.style.add_modifier.contains(Modifier::BOLD));
    assert!(h3.style.add_modifier.contains(Modifier::ITALIC));

    let h4 = find("四级");
    assert!(!h4.style.add_modifier.contains(Modifier::BOLD));
    assert!(h4.style.add_modifier.contains(Modifier::ITALIC));
}

#[test]
fn blockquotes_inherit_body_and_list_markers_split_ordered_and_unordered_styles() {
    let lines = render_markdown_hyperlink_lines(
        "> 引用正文\n\n- 无序\n\n1. 有序",
        Style::default(),
        120,
        None,
    );
    let quote_prefix = lines
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .find(|span| span.content.contains("> "))
        .expect("引用前缀应存在");
    assert_eq!(quote_prefix.style.fg, None);

    let unordered = lines
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .find(|span| span.content.contains("- 无序"))
        .expect("无序列表标记应存在");
    assert_eq!(unordered.style.fg, None);

    let ordered = lines
        .iter()
        .flat_map(|line| line.line.spans.iter())
        .find(|span| span.content.as_ref() == "1. ")
        .expect("有序列表标记应存在");
    assert_eq!(ordered.style.fg, Some(Color::LightBlue));
}
