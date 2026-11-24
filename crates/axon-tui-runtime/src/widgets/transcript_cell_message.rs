//! Message transcript cell family.

use ratatui::{style::Modifier, text::Span};

use crate::{
    app_shell::{
        style_tokens::{MESSAGE_CONTINUATION_PREFIX, MESSAGE_INITIAL_PREFIX},
        syntax_theme::{default_syntax_theme, terminal_background_style},
    },
    surface::cell::RenderCellRecord,
    widgets::{
        terminal_hyperlinks::{HyperlinkLine, prefix_hyperlink_lines},
        transcript_cells::display_width,
        transcript_markdown::render_markdown_hyperlink_lines,
        transcript_style::role_visual,
    },
};

const MESSAGE_TRAILING_GUTTER_WIDTH: u16 = 1;

pub(super) fn render_hyperlink_lines(
    selected: bool,
    line: &RenderCellRecord,
    width: u16,
) -> Vec<HyperlinkLine> {
    render_message_source_hyperlink_lines(message_source(line), line.role, selected, width)
}

pub(crate) fn render_message_source_hyperlink_lines(
    source: &str,
    role: &str,
    selected: bool,
    width: u16,
) -> Vec<HyperlinkLine> {
    let role_visual = role_visual(role);
    render_markdown_lines(
        selected,
        role,
        source,
        role_visual.text_style,
        width,
        Span::styled(MESSAGE_INITIAL_PREFIX, role_visual.marker_style),
    )
}

/// 将已确认不含 Markdown 结构的 source chunk 作为消息续行布局。
pub(crate) fn render_message_source_continuation_hyperlink_lines(
    source: &str,
    role: &str,
    selected: bool,
    width: u16,
) -> Vec<HyperlinkLine> {
    let role_visual = role_visual(role);
    render_markdown_lines(
        selected,
        role,
        source,
        role_visual.text_style,
        width,
        Span::raw(MESSAGE_CONTINUATION_PREFIX),
    )
}

fn render_markdown_lines(
    selected: bool,
    role: &str,
    source: &str,
    text_style: ratatui::style::Style,
    width: u16,
    initial_prefix: Span<'static>,
) -> Vec<HyperlinkLine> {
    let body_width = message_body_width(width);
    let mut lines = prefix_hyperlink_lines(
        render_markdown_hyperlink_lines(
            source,
            text_style,
            body_width,
            Some(default_syntax_theme()),
        ),
        initial_prefix,
        Span::raw(MESSAGE_CONTINUATION_PREFIX),
    );
    let user_style = if role == crate::surface::cell::ROLE_USER {
        Some(terminal_background_style())
    } else {
        None
    };
    for rendered in &mut lines {
        if let Some(user_style) = user_style {
            for span in &mut rendered.line.spans {
                span.style = span.style.patch(user_style);
            }
        }
        if selected {
            for span in &mut rendered.line.spans {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            }
        }
    }
    lines
}

fn message_body_width(width: u16) -> u16 {
    let prefix_width = u16::try_from(display_width(MESSAGE_INITIAL_PREFIX)).unwrap_or(u16::MAX);
    width
        .saturating_sub(prefix_width)
        .saturating_sub(MESSAGE_TRAILING_GUTTER_WIDTH)
        .max(1)
}

fn message_source(line: &RenderCellRecord) -> &str {
    &line.text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::terminal_hyperlinks::HyperlinkLine;
    use ratatui::style::Style;

    fn rendered_text(lines: &[HyperlinkLine]) -> String {
        lines
            .iter()
            .map(|line| {
                line.line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn message_rendering_reserves_the_terminal_trailing_column() {
        let width = 24;
        let cell = RenderCellRecord::agent_message(
            "assistant:test:trailing-gutter",
            "- 在 eaip/packages/admin/src/views/connector/execution/index.vue 中集成 useAgent",
        );

        let lines = render_hyperlink_lines(false, &cell, width);

        assert!(
            lines
                .iter()
                .all(|line| line.line.width() < usize::from(width))
        );
        assert!(rendered_text(&lines).contains("useAgent"));
    }

    #[test]
    fn streaming_source_uses_shared_markdown_renderer() {
        let cell = RenderCellRecord::streaming_agent_message(
            "assistant:test:streaming-table",
            "| 文件 | 状态 |\n| --- | --- |\n| src/main.rs | 已完成 |",
        );

        let text = rendered_text(&render_hyperlink_lines(false, &cell, 80));

        assert!(text.contains("文件"));
        assert!(text.contains("src/main.rs"));
        assert!(!text.contains("| --- | --- |"));
    }

    #[test]
    fn agent_plain_body_inherits_terminal_foreground() {
        let cell = RenderCellRecord::streaming_agent_message(
            "assistant:test:default-foreground",
            "普通内容",
        );

        let lines = render_hyperlink_lines(false, &cell, 80);
        let body_span = lines[0]
            .line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "普通内容")
            .expect("普通正文 span 应存在");

        assert_eq!(body_span.style, Style::default());
    }

    #[test]
    fn streaming_plain_source_keeps_display_layout() {
        let cell = RenderCellRecord::streaming_agent_message(
            "assistant:test:streaming-plain",
            "普通内容\n续行内容",
        );

        let text = rendered_text(&render_hyperlink_lines(false, &cell, 80));

        assert_eq!(text, "• 普通内容\n  续行内容");
    }

    #[test]
    fn streaming_source_task_list_uses_shared_markdown_renderer() {
        let cell = RenderCellRecord::streaming_agent_message(
            "assistant:test:streaming-task",
            "- [x] 任务 1\n- [ ] 任务 2\n- [x] 任务 3",
        );

        let text = rendered_text(&render_hyperlink_lines(false, &cell, 80));

        assert!(text.contains("任务 1"));
        assert!(text.contains("任务 2"));
        assert!(text.contains("任务 3"));
        assert!(!text.contains("[x]"));
        assert!(!text.contains("[ ]"));
    }

    #[test]
    fn streaming_completed_and_resume_sources_render_identically() {
        let source = "# 标题\n\n| 项目 | 状态 |\n| --- | --- |\n| renderer | ✅ 完成 |\n\n```rust\nfn main() {}\n```";
        let streaming =
            RenderCellRecord::streaming_agent_message("assistant:test:uniform-stream", source);
        let completed = RenderCellRecord::agent_message("assistant:test:uniform-complete", source);

        let streaming_text = rendered_text(&render_hyperlink_lines(false, &streaming, 80));
        let completed_text = rendered_text(&render_hyperlink_lines(false, &completed, 80));

        assert_eq!(streaming_text, completed_text);
        assert!(!streaming_text.contains("# 标题"));
        assert!(!streaming_text.contains("| --- | --- |"));
        assert!(!streaming_text.contains("```rust"));
    }

    #[test]
    fn resize_reflows_from_source_without_overlap_or_stale_lines() {
        let source = concat!(
            "已完成 Markdown 排版重构，中文正文必须在窗口变化后重新换行。\n\n",
            "- [源码](crates/axon-tui-runtime/src/widgets/transcript_cell_message.rs:1) 保持链接元数据\n",
            "- streaming、final 与 resume 共用 source-backed renderer",
        );
        let cell = RenderCellRecord::agent_message("assistant:test:resize", source);

        let narrow = render_hyperlink_lines(false, &cell, 40);
        let medium = render_hyperlink_lines(false, &cell, 80);
        let wide = render_hyperlink_lines(false, &cell, 120);
        let narrow_again = render_hyperlink_lines(false, &cell, 40);

        for (width, lines) in [(40_u16, &narrow), (80_u16, &medium), (120_u16, &wide)] {
            assert!(
                lines
                    .iter()
                    .all(|line| line.line.width() <= usize::from(width)),
                "窗口宽度 {width} 下存在横向溢出：{lines:#?}"
            );
            assert!(rendered_text(lines).starts_with(MESSAGE_INITIAL_PREFIX));
        }
        assert_eq!(narrow, narrow_again, "resize 回原宽度必须得到相同物理行");
        assert!(narrow.len() >= medium.len() && medium.len() >= wide.len());
        assert!(narrow.iter().skip(1).all(|line| {
            let text = line
                .line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            text.is_empty() || text.starts_with(MESSAGE_CONTINUATION_PREFIX)
        }));
    }
}
