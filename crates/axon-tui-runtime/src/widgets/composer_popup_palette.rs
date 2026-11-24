//! Composer mention popup 渲染组件。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::composer_popup::{ComposerPopupItem, ComposerPopupMentionType},
    widgets::transcript_style::role_style,
};

const POPUP_PANEL_MAX_HEIGHT: u16 = 9;
const POPUP_PANEL_FOOTER_HEIGHT: u16 = 1;
const POPUP_HORIZONTAL_PADDING: u16 = 2;

pub(crate) struct ComposerPopupPalette<'a> {
    pub title: &'a str,
    pub items: &'a [ComposerPopupItem],
    pub selected: usize,
}

impl ComposerPopupPalette<'_> {
    pub(crate) fn required_height(item_count: usize) -> u16 {
        if item_count == 0 {
            return 0;
        }
        let max_items = POPUP_PANEL_MAX_HEIGHT.saturating_sub(POPUP_PANEL_FOOTER_HEIGHT);
        let visible = item_count.min(usize::from(max_items));
        u16::try_from(visible).unwrap_or(u16::MAX) + POPUP_PANEL_FOOTER_HEIGHT
    }

    fn build_lines(&self, visible_count: usize) -> Vec<Line<'static>> {
        self.visible_items(visible_count)
            .map(|(index, item)| popup_line(item, index == self.selected))
            .collect()
    }

    fn visible_items(
        &self,
        visible_count: usize,
    ) -> impl Iterator<Item = (usize, &ComposerPopupItem)> + '_ {
        let start = selected_window_start(self.selected, self.items.len(), visible_count);
        self.items
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_count)
    }
}

impl Widget for ComposerPopupPalette<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 || self.items.is_empty() {
            return;
        }
        let list_height = area.height.saturating_sub(POPUP_PANEL_FOOTER_HEIGHT);
        if list_height == 0 {
            return;
        }
        let visible_count = self
            .items
            .len()
            .min(usize::from(
                POPUP_PANEL_MAX_HEIGHT.saturating_sub(POPUP_PANEL_FOOTER_HEIGHT),
            ))
            .min(list_height as usize);
        if visible_count == 0 {
            return;
        }
        let list_area = Rect {
            x: area.x.saturating_add(POPUP_HORIZONTAL_PADDING),
            y: area.y,
            width: area.width.saturating_sub(POPUP_HORIZONTAL_PADDING),
            height: list_height,
        };
        for (offset, line) in self.build_lines(visible_count).into_iter().enumerate() {
            if let Ok(y_offset) = u16::try_from(offset) {
                line.render(
                    Rect {
                        x: list_area.x,
                        y: list_area.y.saturating_add(y_offset),
                        width: list_area.width,
                        height: 1,
                    },
                    buf,
                );
            }
        }
        let input_status_y = area.y.saturating_add(list_height);
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                self.title.to_owned(),
                role_style(TranscriptColorRole::Muted),
            ),
            Span::styled(
                " · Enter 插入 · Esc 关闭",
                role_style(TranscriptColorRole::Muted),
            ),
        ])
        .render(
            Rect {
                x: area.x,
                y: input_status_y,
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

fn selected_window_start(selected: usize, total: usize, visible_count: usize) -> usize {
    if visible_count == 0 || total <= visible_count {
        return 0;
    }
    selected
        .min(total.saturating_sub(1))
        .saturating_add(1)
        .saturating_sub(visible_count)
}

fn popup_line(item: &ComposerPopupItem, selected: bool) -> Line<'static> {
    let base_style = if selected {
        selected_style().add_modifier(Modifier::BOLD)
    } else {
        role_style(TranscriptColorRole::Assistant)
    };
    let muted_style = if selected {
        selected_style()
    } else {
        role_style(TranscriptColorRole::Muted)
    };
    let name_style = mention_name_style(item.mention_type, selected);
    let mut spans = Vec::new();
    spans.push(Span::styled(" ", base_style));
    spans.extend(highlighted_value(item, name_style));
    spans.push(Span::styled("  ", muted_style));
    spans.push(Span::styled(
        format!("{:<9}", item.mention_type.label()),
        muted_style,
    ));
    spans.push(Span::styled(" ", muted_style));
    spans.push(Span::styled(item.description.clone(), base_style));
    Line::from(spans)
}

fn mention_name_style(mention_type: ComposerPopupMentionType, selected: bool) -> Style {
    if selected {
        return selected_style().add_modifier(Modifier::BOLD);
    }
    role_style(match mention_type {
        ComposerPopupMentionType::Skill => TranscriptColorRole::Muted,
        ComposerPopupMentionType::File => TranscriptColorRole::Accent,
        ComposerPopupMentionType::Image => TranscriptColorRole::Loading,
        ComposerPopupMentionType::Directory => TranscriptColorRole::Assistant,
        ComposerPopupMentionType::Shell => TranscriptColorRole::ToolCompleted,
        ComposerPopupMentionType::Connector => TranscriptColorRole::Accent,
    })
    .add_modifier(match mention_type {
        ComposerPopupMentionType::Skill | ComposerPopupMentionType::Directory => Modifier::empty(),
        _ => Modifier::BOLD,
    })
}

fn selected_style() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

fn highlighted_value(item: &ComposerPopupItem, base_style: Style) -> Vec<Span<'static>> {
    let Some(indices) = item.match_indices.as_ref() else {
        return vec![Span::styled(item.value.clone(), base_style)];
    };
    let mut index_iter = indices.iter().peekable();
    item.value
        .chars()
        .enumerate()
        .map(|(char_index, ch)| {
            let mut style = base_style;
            if index_iter.peek().is_some_and(|index| **index == char_index) {
                index_iter.next();
                style = style.add_modifier(Modifier::UNDERLINED);
            }
            Span::styled(ch.to_string(), style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::*;

    #[test]
    fn popup_palette_renders_type_labels_and_items() {
        let items = [
            ComposerPopupItem {
                value: "@crates/axon/src/model_router.rs".to_owned(),
                description: "file".to_owned(),
                insert_text: "@crates/axon/src/model_router.rs".to_owned(),
                target: "file://crates/axon/src/model_router.rs".to_owned(),
                mention_type: ComposerPopupMentionType::File,
                match_indices: Some(vec![18, 19]),
                score: 1_000,
            },
            ComposerPopupItem {
                value: "$frontend-design".to_owned(),
                description: "skill".to_owned(),
                insert_text: "$frontend-design ".to_owned(),
                target: "skill://frontend-design".to_owned(),
                mention_type: ComposerPopupMentionType::Skill,
                match_indices: Some(vec![1, 9]),
                score: 500,
            },
        ];
        let area = Rect::new(0, 0, 80, 5);
        let mut buffer = Buffer::empty(area);

        ComposerPopupPalette {
            title: "Files",
            items: &items,
            selected: 0,
        }
        .render(area, &mut buffer);

        let text = buffer_text(&buffer);
        assert!(text.contains("Files"));
        assert!(text.contains("@crates/axon/src/model_router.rs"));
        assert!(text.contains("$frontend-design"));
        assert!(text.contains("file"));
        assert!(text.contains("skill"));
        assert!(!text.contains("┌"));
        assert!(!text.contains("┐"));
        assert!(!text.contains("└"));
        assert!(!text.contains("┘"));
    }

    #[test]
    fn popup_palette_windows_selected_item_without_border() {
        let items = (0..10)
            .map(|index| ComposerPopupItem {
                value: format!("$skill-{index}"),
                description: "skill".to_owned(),
                insert_text: format!("$skill-{index} "),
                target: format!("skill://skill-{index}"),
                mention_type: ComposerPopupMentionType::Skill,
                match_indices: None,
                score: 1_000 - index,
            })
            .collect::<Vec<_>>();
        let area = Rect::new(0, 0, 40, 9);
        let mut buffer = Buffer::empty(area);

        ComposerPopupPalette {
            title: "Skills",
            items: &items,
            selected: 9,
        }
        .render(area, &mut buffer);

        let text = buffer_text(&buffer);
        assert!(text.contains("$skill-2"));
        assert!(text.contains("$skill-9"));
        assert!(!text.contains("$skill-1"));
        assert!(!text.contains("┌"));
        assert!(!text.contains("┘"));
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }
}
