use super::*;

#[test]
fn releases_plain_paragraph_at_line_boundary() {
    let source = "before\nplain text\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        source.len()
    );
}

#[test]
fn holds_unclosed_fenced_code_from_opening_fence() {
    let source = "before\n```rust\nfn main() {}\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n".len()
    );
}

#[test]
fn releases_fenced_code_after_closing_fence() {
    let source = "before\n```rust\nfn main() {}\n```\nafter\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        source.len()
    );
}

#[test]
fn holds_table_until_blank_boundary() {
    let source = "before\n| a | b |\n| --- | --- |\n| 1 | 2 |\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n".len()
    );
    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(&(source.to_owned() + "\n")),
        source.len() + 1
    );
}

#[test]
fn holds_potential_table_header_until_next_line_decides() {
    let source = "before\n| a | b |\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n".len()
    );
    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(&(source.to_owned() + "not table\n")),
        source.len() + "not table\n".len()
    );
}

#[test]
fn releases_potential_table_header_after_blank_boundary() {
    let source = "before\n| a | b |\n\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        source.len()
    );
}

#[test]
fn holds_incomplete_cjk_heading_until_line_boundary() {
    for source in ["before\n##标题", "before\n##\u{3000}标题"] {
        assert_eq!(
            StreamingMarkdownBoundary::stable_source_len(source),
            "before\n".len()
        );
        let completed = format!("{source}\n");
        assert_eq!(
            StreamingMarkdownBoundary::stable_source_len(&completed),
            completed.len()
        );
    }
}

#[test]
fn holds_unclosed_inline_markers_and_releases_closed_forms() {
    for source in [
        "before\n*强调\n",
        "before\n_强调\n",
        "before\n**重点\n",
        "before\n`code\n",
        "before\n[链接\n",
    ] {
        assert_eq!(
            StreamingMarkdownBoundary::stable_source_len(source),
            "before\n".len()
        );
    }
    for source in [
        "before\n*强调*\n",
        "before\n_强调_\n",
        "before\n**重点**\n",
        "before\n`code`\n",
        "before\n[链接](https://example.com)\n",
    ] {
        assert_eq!(
            StreamingMarkdownBoundary::stable_source_len(source),
            source.len()
        );
    }
}

#[test]
fn releases_escaped_or_block_terminated_literal_markers() {
    for source in [
        "before\n\\*\\*字面量\n",
        "before\n\\`字面量\n",
        "before\n\\[字面量\n",
        "before\n**最终字面量\n\n",
    ] {
        assert_eq!(
            StreamingMarkdownBoundary::stable_source_len(source),
            source.len()
        );
    }
}

#[test]
fn holds_blockquoted_table_until_blank_boundary() {
    let source = "> intro\n> | a | b |\n> | --- | --- |\n> | 1 | 2 |\n";

    assert_eq!(StreamingMarkdownBoundary::stable_source_len(source), 0);
    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(&(source.to_owned() + "\n")),
        source.len() + 1
    );
}

#[test]
fn holds_list_until_blank_boundary() {
    let source = "before\n- item\n  continuation\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n".len()
    );
    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(&(source.to_owned() + "\n")),
        source.len() + 1
    );
}

#[test]
fn releases_completed_list_items_and_holds_only_last_item() {
    let source = "before\n- first\n- second\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n- first\n".len()
    );
}

#[test]
fn nested_list_holds_outer_last_item_not_inner_last_item() {
    let source = "before\n- first\n- second\n  - child\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n- first\n".len()
    );
}

#[test]
fn releases_html_block_after_explicit_closing_tag() {
    let source = "before\n<script>\nconsole.log(1);\n</script>\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        source.len()
    );
}

#[test]
fn keeps_non_raw_html_block_waiting_for_blank_boundary() {
    let source = "before\n<div>\ncontent\n</div>\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n".len()
    );
}

#[test]
fn releases_html_comment_after_explicit_close() {
    let source = "before\n<!-- note -->\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        source.len()
    );
}

#[test]
fn holds_html_block_until_blank_boundary() {
    let source = "before\n<div>\ncontent\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n".len()
    );
    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(&(source.to_owned() + "\n")),
        source.len() + 1
    );
}

#[test]
fn holds_indented_code_until_blank_boundary() {
    let source = "before\n\n    let x = 1;\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\n\n".len()
    );
}

#[test]
fn treats_single_crlf_as_line_boundary_not_blank_boundary() {
    let source = "before\r\n- item\r\n";

    assert_eq!(
        StreamingMarkdownBoundary::stable_source_len(source),
        "before\r\n".len()
    );
}

#[test]
fn incrementally_confirms_structured_markdown_without_rescanning_committed_history() {
    const LINE_COUNT: usize = 1_000;
    let mut boundary = IncrementalStreamingMarkdownBoundary::default();
    let mut source = String::new();

    for index in 0..LINE_COUNT {
        let chunk = format!("**重点第 {index} 行**\n");
        source.push_str(&chunk);
        let update = boundary.append_source(&source, &chunk);
        assert_eq!(
            update.stable_source_len,
            StreamingMarkdownBoundary::stable_source_len(&source)
        );
    }

    assert!(boundary.parsed_source_bytes() <= source.len());
}

#[test]
fn incrementally_confirms_list_items_without_rescanning_committed_history() {
    const LINE_COUNT: usize = 1_000;
    let mut boundary = IncrementalStreamingMarkdownBoundary::default();
    let mut source = String::new();

    for index in 0..LINE_COUNT {
        let chunk = format!("- 列表第 {index} 项\n");
        source.push_str(&chunk);
        let update = boundary.append_source(&source, &chunk);
        assert_eq!(
            update.stable_source_len,
            StreamingMarkdownBoundary::stable_source_len(&source)
        );
    }

    assert!(boundary.parsed_source_bytes() <= source.len().saturating_mul(2));
}

#[test]
fn markdown_chunk_parses_only_the_unresolved_tail() {
    let mut boundary = IncrementalStreamingMarkdownBoundary::default();
    let mut source = String::from("普通文本\n");
    boundary.append_source(&source, &source.clone());
    let chunk = "**未闭合强调\n";
    source.push_str(chunk);

    let update = boundary.append_source(&source, chunk);

    assert_eq!(
        update.stable_source_len,
        StreamingMarkdownBoundary::stable_source_len(&source)
    );
    assert_eq!(boundary.parsed_source_bytes(), chunk.len());
}
