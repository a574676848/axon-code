//! 会话目录弹窗渲染。
//!
//! 每条候选一行：标记列（箭头）在前，标题居中偏左，来源贴住行尾；选中项只把内容区
//! 整条铺满反色，标记列始终是正常背景，上下移动时读者看到的是一条内容条在走。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::bottom_view::SessionsView,
    widgets::transcript_style::{
        role_style, selectable_row_marker_style, selectable_row_selected_style,
        selectable_row_style, strong_role_style,
    },
};

const PREFIX_SELECTED: &str = "› ";
const PREFIX_IDLE: &str = "  ";
/// 标题与来源之间的最小间隔。
const DESCRIPTION_GAP: usize = 2;
/// 当前工作区没有会话时的说明。
const EMPTY_TEXT: &str = "当前工作区还没有会话，直接输入任务即可开始第一条。";

pub(super) fn render(view: &SessionsView, area: Rect, buf: &mut Buffer) {
    if area.height == 0 {
        return;
    }
    buf.set_line(
        area.x,
        area.y,
        &Line::from(vec![
            Span::styled(
                view.title().to_owned(),
                strong_role_style(TranscriptColorRole::Loading),
            ),
            Span::raw(" "),
            Span::styled(
                view.subtitle().to_owned(),
                role_style(TranscriptColorRole::Muted),
            ),
        ]),
        area.width,
    );

    // 面板高度由输入坞决定，可能小于弹窗申请的容量：按真实高度开窗，选中项才不会被裁掉。
    let capacity = view.visible_capacity(area.height);
    if capacity == 0 {
        return;
    }
    if view.items().is_empty() {
        // 列表按工作区收敛：这里空着不是加载失败，而是这个工作区确实还没有会话。
        buf.set_line(
            area.x,
            area.y.saturating_add(1),
            &Line::from(Span::styled(
                EMPTY_TEXT.to_owned(),
                role_style(TranscriptColorRole::Muted),
            )),
            area.width,
        );
        return;
    }
    let start = view.window_start(capacity);
    let row_width = usize::from(area.width);
    let prefix_width = UnicodeWidthStr::width(PREFIX_SELECTED);
    let mut row = area.y.saturating_add(1);
    for (index, item) in view.items().iter().enumerate().skip(start).take(capacity) {
        let selected = index == view.selected();
        let prefix = if selected {
            PREFIX_SELECTED
        } else {
            PREFIX_IDLE
        };
        // 标题与来源同一行：来源贴住行尾，标题按剩余宽度裁剪，两条信息都不丢。
        let description = item.description.trim();
        let description_width = UnicodeWidthStr::width(description);
        let gap_width = if description.is_empty() {
            0
        } else {
            DESCRIPTION_GAP
        };
        let label_budget = row_width
            .saturating_sub(prefix_width)
            .saturating_sub(gap_width)
            .saturating_sub(description_width);
        let label = clip_with_ellipsis(&item.label, label_budget);
        let label_width = UnicodeWidthStr::width(label.as_str());
        // 高亮只管内容区：标记列（箭头所在的 leading 列）不参与反色，
        // 选中与未选中都用同一种标记色，上下移动时读者只看到内容条在走。
        let bar_style = selectable_row_selected_style();
        let content_style = if selected {
            bar_style
        } else if item.archived {
            selectable_row_style(TranscriptColorRole::Muted)
        } else {
            selectable_row_style(TranscriptColorRole::Assistant)
        };
        let description_style = if selected {
            bar_style
        } else {
            selectable_row_style(TranscriptColorRole::Muted)
        };
        let mut spans = vec![
            Span::styled(prefix, selectable_row_marker_style()),
            Span::styled(label, content_style),
        ];
        let mut used_width = prefix_width.saturating_add(label_width);
        if !description.is_empty() {
            let gap = row_width
                .saturating_sub(used_width)
                .saturating_sub(description_width)
                .max(gap_width);
            spans.push(Span::styled(" ".repeat(gap), description_style));
            spans.push(Span::styled(description.to_owned(), description_style));
            used_width = used_width
                .saturating_add(gap)
                .saturating_add(description_width);
        }
        // 选中行的内容条铺到行尾：整条一起移动，行尾不会留下半亮的残段。
        if selected {
            let padding = row_width.saturating_sub(used_width);
            if padding > 0 {
                spans.push(Span::styled(" ".repeat(padding), bar_style));
            }
        }
        buf.set_line(area.x, row, &Line::from(spans), area.width);
        row = row.saturating_add(1);
    }
}

/// 按显示宽度裁剪并补省略号。
///
/// `buf.set_line` 只会硬截断：超宽标题被切成半截词，用户看不出后面还有内容。
/// 中文与全角标点按 2 列计，因此必须按字符宽度累计，不能按字节或字符数切。
fn clip_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_owned();
    }
    let budget = max_width.saturating_sub(1);
    let mut used = 0;
    let mut clipped = String::new();
    for character in text.chars() {
        let width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + width > budget {
            break;
        }
        used += width;
        clipped.push(character);
    }
    clipped.push('…');
    clipped
}
