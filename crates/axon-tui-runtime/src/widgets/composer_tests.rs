use ratatui::{buffer::Buffer, layout::Rect, style::Modifier, widgets::Widget};

use super::{Composer, ComposerDraft, ComposerDraftElement, ComposerDraftElementKind};
use crate::app_shell::style_tokens::{TranscriptColorRole, transcript_palette};

#[test]
fn composer_renders_queued_preview_above_input_divider() {
    let area = Rect::new(0, 0, 80, 8);
    let mut buffer = Buffer::empty(area);

    Composer {
        draft: ComposerDraft::plain("继续编辑当前输入"),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: Some("1. next high value task\n2. follow up slash command"),
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let text = buffer_text(&buffer);
    let normalized = normalized_text(&text);
    assert!(normalized.contains("待发送"));
    assert!(!normalized.contains("Esc中断"));
    assert!(normalized.contains("Alt+E编辑最近一条"));
    assert!(text.contains("1. next high value task"));
    assert!(text.contains("2. follow up slash command"));
    assert!(!normalized.contains("生成回复中"));
    let rendered_rows = (0..area.height)
        .map(|row| row_text(&buffer, row))
        .collect::<Vec<_>>();
    assert!(rendered_rows[2].contains("2. follow up slash command"));
    assert!(rendered_rows[3].starts_with('─'));
    assert!(
        normalized_text(&rendered_rows[4]).contains("继续编辑当前输入"),
        "rendered_rows={rendered_rows:?}"
    );
}

#[test]
fn composer_renders_placeholder_for_empty_draft() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);

    Composer {
        draft: ComposerDraft::plain(""),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let text = buffer_text(&buffer);
    assert!(normalized_text(&text).contains("输入任务"));
    assert!(!text.contains("Ready"));
}

#[test]
fn composer_renders_single_line_draft_in_compact_area() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);

    Composer {
        draft: ComposerDraft::plain("你好"),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let text = buffer_text(&buffer);
    assert!(normalized_text(&text).contains("你好"));
    assert!(!normalized_text(&text).contains("Commands"));
}

#[test]
fn composer_clears_stale_content_when_draft_shrinks_on_preserved_buffer() {
    let area = Rect::new(0, 0, 24, 4);
    let mut buffer = Buffer::empty(area);

    Composer {
        draft: ComposerDraft::plain("AXON_IMPLEMENTATION_PLAN.md"),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);
    Composer {
        draft: ComposerDraft::plain("AXON_PLAN.md"),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    assert_eq!(row_text(&buffer, 1).trim_end(), "› AXON_PLAN.md");
    assert!(row_text(&buffer, 2).trim().is_empty());
}

#[test]
fn composer_soft_wraps_draft_at_available_input_width() {
    let area = Rect::new(0, 0, 8, 4);
    let mut buffer = Buffer::empty(area);

    Composer {
        draft: ComposerDraft::plain("abcdefghi"),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    assert_eq!(row_text(&buffer, 1), "› abcdef");
    assert_eq!(row_text(&buffer, 2), "  ghi   ");
    assert_eq!(row_text(&buffer, 3), "────────");
}

#[test]
fn composer_does_not_render_working_input_status() {
    let area = Rect::new(0, 0, 80, 4);
    let mut buffer = Buffer::empty(area);

    Composer {
        draft: ComposerDraft::plain("继续输入下一轮任务"),
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let text = buffer_text(&buffer);
    let normalized = normalized_text(&text);
    assert!(!text.contains("⠋"));
    assert!(!normalized.contains("调用"));
    assert!(!normalized.contains("Esc中断"));
    assert!(!normalized.contains("命令·builtin.shell_command"));
    assert!(normalized.contains("继续输入下一轮任务"));
    assert!(!normalized.contains("Commands"));
}

#[test]
fn composer_renders_mention_elements_with_structured_style() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);
    let elements = [ComposerDraftElement {
        range: 2..9,
        kind: ComposerDraftElementKind::Mention('$'),
    }];

    Composer {
        draft: ComposerDraft {
            text: "用 $review 检查",
            elements: &elements,
        },
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let mention_x = find_symbol_x(&buffer, 1, "$ ").or_else(|| find_symbol_x(&buffer, 1, "$"));
    let mention_x = mention_x.expect("mention sigil should render");
    let palette = transcript_palette();
    assert_eq!(
        buffer[(mention_x, 1)].fg,
        palette.color(TranscriptColorRole::Muted)
    );
    assert_eq!(
        buffer[(mention_x.saturating_sub(1), 1)].fg,
        palette.color(TranscriptColorRole::Assistant)
    );
}

#[test]
fn composer_preserves_mention_style_when_tail_lines_are_visible() {
    let area = Rect::new(0, 0, 80, 4);
    let mut buffer = Buffer::empty(area);
    let elements = [ComposerDraftElement {
        range: 7..15,
        kind: ComposerDraftElementKind::Mention('@'),
    }];

    Composer {
        draft: ComposerDraft {
            text: "第一行\n查看 @main.rs",
            elements: &elements,
        },
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let mention_x = find_symbol_x(&buffer, 2, "@").expect("mention sigil should render");
    assert_eq!(
        buffer[(mention_x, 2)].fg,
        transcript_palette().color(TranscriptColorRole::Accent)
    );
    assert!(buffer[(mention_x, 2)].modifier.contains(Modifier::BOLD));
}

#[test]
fn composer_renders_attachment_elements_with_attachment_style() {
    let area = Rect::new(0, 0, 80, 3);
    let mut buffer = Buffer::empty(area);
    let elements = [
        ComposerDraftElement {
            range: 2..22,
            kind: ComposerDraftElementKind::File,
        },
        ComposerDraftElement {
            range: 23..43,
            kind: ComposerDraftElementKind::Image,
        },
    ];

    Composer {
        draft: ComposerDraft {
            text: "看 [File #1] report.txt [Image #2] chart.png",
            elements: &elements,
        },
        placeholder: "输入任务",
        command_suggestions: &[],
        popup_items: &[],
        suggestion_title: "Commands",
        selected_command_suggestion: 0,
        queued_preview: None,
        draft_scroll_offset: 0,
    }
    .render(area, &mut buffer);

    let file_x = find_symbol_x(&buffer, 1, "[File").expect("file placeholder should render");
    let image_x = find_symbol_x(&buffer, 1, "[Image").expect("image placeholder should render");

    assert_eq!(
        buffer[(file_x, 1)].fg,
        transcript_palette().color(TranscriptColorRole::Accent)
    );
    assert_eq!(
        buffer[(image_x, 1)].fg,
        transcript_palette().color(TranscriptColorRole::Loading)
    );
    assert!(buffer[(file_x, 1)].modifier.contains(Modifier::BOLD));
    assert!(buffer[(image_x, 1)].modifier.contains(Modifier::BOLD));
}

fn find_symbol_x(buffer: &Buffer, y: u16, symbol: &str) -> Option<u16> {
    let end = buffer.area.x.saturating_add(buffer.area.width);
    (buffer.area.x..end).find(|x| {
        let text = (*x..end)
            .map(|col| buffer[(col, y)].symbol())
            .collect::<String>();
        text.starts_with(symbol)
    })
}

fn buffer_text(buffer: &Buffer) -> String {
    buffer
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect::<String>()
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (buffer.area.x..buffer.area.x + buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect::<String>()
}

fn normalized_text(text: &str) -> String {
    text.split_whitespace().collect::<String>()
}
