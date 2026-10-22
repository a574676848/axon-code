use std::{convert::Infallible, io};

use ratatui::{
    backend::{Backend, TestBackend},
    buffer::Cell,
    layout::{Position, Rect, Size},
    style::Style,
};

use super::{
    custom_terminal::Terminal,
    state::visible_text_selection::{VisibleTextSelectionState, VisibleTextSnapshot},
};
use crate::surface::display_width::DisplayWidthText;

#[test]
fn fullscreen_terminal_uses_entire_backend_area() {
    let backend = ResizableCaptureBackend::new(80, 24);
    let terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

    assert_eq!(terminal.viewport_area, Rect::new(0, 0, 80, 24));
}

#[test]
fn fullscreen_viewport_resizes_with_backend_size() {
    let backend = ResizableCaptureBackend::new(80, 24);
    let mut terminal = Terminal::new_for_test(backend, 24).expect("测试终端应可创建");

    terminal.backend_mut().inner.resize(100, 30);
    terminal
        .update_fullscreen_viewport()
        .expect("全屏 viewport 更新应成功");

    assert_eq!(terminal.viewport_area, Rect::new(0, 0, 100, 30));
    assert_eq!(terminal.backend().draw_calls, 0);
    assert_eq!(terminal.backend().clear_calls, 1);
}

#[test]
fn preserving_previous_frame_keeps_unchanged_transcript_cells() {
    let mut terminal = Terminal::new_for_test(TestBackend::new(20, 4), 4).expect("终端应可创建");
    terminal
        .draw(|frame| {
            frame
                .buffer_mut()
                .set_string(0, 0, "历史输出", Style::default());
        })
        .expect("首帧应成功渲染");

    terminal
        .draw_preserving_previous(|frame| {
            frame
                .buffer_mut()
                .set_string(0, 3, "输入区", Style::default());
        })
        .expect("复用上一帧应成功渲染");

    let snapshot = terminal.visible_text_snapshot();
    let mut selection = VisibleTextSelectionState::default();
    selection.update_snapshot(snapshot);
    selection.begin(0, 0);
    assert!(selection.update(3, 3));
    let selected = selection.finish().expect("应能读取复用帧");
    assert!(selected.contains("历史输出"));
    assert!(selected.contains("输入"));
}

#[test]
fn visible_snapshot_copies_continuous_cjk_and_ascii_without_continuation_spaces() {
    const TEXT: &str = "你先使用ova_mcp检索epaas开发";
    let snapshot = rendered_snapshot(&[TEXT]);
    let end_column = terminal_width(TEXT).saturating_sub(1);

    assert_eq!(copy_range(snapshot, (0, 0), (0, end_column)), TEXT);
}

#[test]
fn active_drag_keeps_mouse_down_snapshot_across_redraws() {
    const TEXT: &str = "EAIP智能打单连接器_业务场景汇报.html";
    let mut selection = VisibleTextSelectionState::default();
    selection.update_snapshot(rendered_snapshot(&[TEXT]));
    selection.begin(0, 0);
    selection.update_snapshot(rendered_snapshot(&["EAIP智能打单连接器_.html"]));
    assert!(selection.update(0, terminal_width(TEXT).saturating_sub(1)));

    assert_eq!(selection.finish(), Some(TEXT.to_owned()));
}

#[test]
fn visible_snapshot_preserves_real_spaces_between_wide_graphemes() {
    const TEXT: &str = "你好 世界";
    let snapshot = rendered_snapshot(&[TEXT]);
    let end_column = terminal_width(TEXT).saturating_sub(1);

    assert_eq!(copy_range(snapshot, (0, 0), (0, end_column)), TEXT);
}

#[test]
fn visible_snapshot_selects_complete_unicode_graphemes_from_wide_trailing_cells() {
    const PREFIX: &str = "A";
    const FAMILY: &str = "👨‍👩‍👧‍👦";
    const COMBINED: &str = "e\u{301}";
    const CJK: &str = "中";
    let text = format!("{PREFIX}{FAMILY}{COMBINED}{CJK}Z");
    let snapshot = rendered_snapshot(&[text.as_str()]);
    let start_column = terminal_width(PREFIX)
        .saturating_add(terminal_width(FAMILY))
        .saturating_sub(1);
    let end_column = terminal_width(&format!("{PREFIX}{FAMILY}{COMBINED}{CJK}")).saturating_sub(1);

    assert_eq!(
        copy_range(snapshot, (0, start_column), (0, end_column)),
        format!("{FAMILY}{COMBINED}{CJK}")
    );
}

fn rendered_snapshot(lines: &[&str]) -> VisibleTextSnapshot {
    let width = lines
        .iter()
        .map(|line| terminal_width(line))
        .max()
        .unwrap_or(0);
    let height = u16::try_from(lines.len()).expect("测试行数应适配终端高度");
    let mut terminal =
        Terminal::new_for_test(TestBackend::new(width, height), height).expect("测试终端应可创建");
    terminal
        .draw(|frame| {
            for (row, line) in lines.iter().enumerate() {
                frame.buffer_mut().set_string(
                    0,
                    u16::try_from(row).expect("测试行号应适配终端坐标"),
                    *line,
                    Style::default(),
                );
            }
        })
        .expect("测试文本应成功渲染");
    terminal.visible_text_snapshot()
}

fn copy_range(snapshot: VisibleTextSnapshot, start: (u16, u16), end: (u16, u16)) -> String {
    let mut selection = VisibleTextSelectionState::default();
    selection.update_snapshot(snapshot);
    selection.begin(start.0, start.1);
    assert!(selection.update(end.0, end.1));
    selection.finish().expect("拖拽选区应生成文本")
}

fn terminal_width(text: &str) -> u16 {
    u16::try_from(DisplayWidthText::width(text)).expect("测试文本宽度应适配终端")
}

struct ResizableCaptureBackend {
    inner: TestBackend,
    draw_calls: usize,
    clear_calls: usize,
}

impl ResizableCaptureBackend {
    fn new(width: u16, height: u16) -> Self {
        Self {
            inner: TestBackend::new(width, height),
            draw_calls: 0,
            clear_calls: 0,
        }
    }
}

impl io::Write for ResizableCaptureBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for ResizableCaptureBackend {
    type Error = Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.draw_calls = self.draw_calls.saturating_add(1);
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> Result<(), Self::Error> {
        self.clear_calls = self.clear_calls.saturating_add(1);
        self.inner.clear_region(clear_type)
    }

    fn append_lines(&mut self, line_count: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(line_count)
    }

    fn scroll_region_up(
        &mut self,
        region: std::ops::Range<u16>,
        scroll_by: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_up(region, scroll_by)
    }

    fn scroll_region_down(
        &mut self,
        region: std::ops::Range<u16>,
        scroll_by: u16,
    ) -> Result<(), Self::Error> {
        self.inner.scroll_region_down(region, scroll_by)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }
}
