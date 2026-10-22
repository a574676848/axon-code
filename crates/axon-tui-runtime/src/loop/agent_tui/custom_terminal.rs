//! Agent TUI 专用全屏终端适配器。
//!
//! 终端只负责全屏 buffer 绘制与光标同步；历史滚动由 RenderViewport 状态管理。

use ratatui::{
    backend::{Backend, ClearType},
    buffer::Buffer,
    layout::{Position, Rect},
    widgets::Widget,
};

use super::state::visible_text_selection::VisibleTextSnapshot;
use crate::{
    surface::terminal_capabilities::TerminalCompatibilityPolicy,
    widgets::{TerminalHyperlinkSurface, terminal_frame_diff},
};

/// 单帧渲染上下文。
pub(super) struct Frame<'a> {
    viewport_area: Rect,
    buffer: &'a mut Buffer,
    hyperlinks: &'a mut TerminalHyperlinkSurface,
    cursor_position: Option<Position>,
}

impl Frame<'_> {
    pub(super) const fn area(&self) -> Rect {
        self.viewport_area
    }

    pub(super) fn buffer_mut(&mut self) -> &mut Buffer {
        self.buffer
    }

    pub(super) fn surface_mut(&mut self) -> (&mut Buffer, &mut TerminalHyperlinkSurface) {
        (&mut *self.buffer, &mut *self.hyperlinks)
    }

    pub(super) fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub(super) fn set_cursor_position<P: Into<Position>>(&mut self, position: P) {
        self.cursor_position = Some(position.into());
    }
}

/// 支持全屏 AppShell 绘制的最小终端包装。
pub(super) struct Terminal<B>
where
    B: Backend,
{
    backend: B,
    buffers: [Buffer; 2],
    hyperlink_surfaces: [TerminalHyperlinkSurface; 2],
    current: usize,
    hidden_cursor: bool,
    pub(super) viewport_area: Rect,
}

impl<B> Terminal<B>
where
    B: Backend,
{
    pub(super) fn new(mut backend: B, _initial_height: u16) -> Result<Self, B::Error> {
        let viewport_area = fullscreen_viewport(backend.size()?);
        backend.hide_cursor()?;
        Ok(Self {
            backend,
            buffers: [Buffer::empty(viewport_area), Buffer::empty(viewport_area)],
            hyperlink_surfaces: [
                TerminalHyperlinkSurface::default(),
                TerminalHyperlinkSurface::default(),
            ],
            current: 0,
            hidden_cursor: true,
            viewport_area,
        })
    }

    #[cfg(test)]
    pub(super) fn new_for_test(backend: B, _initial_height: u16) -> Result<Self, B::Error> {
        let viewport_area = fullscreen_viewport(backend.size()?);
        Ok(Self {
            backend,
            buffers: [Buffer::empty(viewport_area), Buffer::empty(viewport_area)],
            hyperlink_surfaces: [
                TerminalHyperlinkSurface::default(),
                TerminalHyperlinkSurface::default(),
            ],
            current: 0,
            hidden_cursor: false,
            viewport_area,
        })
    }

    #[cfg(test)]
    pub(super) const fn backend(&self) -> &B {
        &self.backend
    }

    pub(super) fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub(super) fn size(&self) -> Result<ratatui::layout::Size, B::Error> {
        self.backend.size()
    }

    pub(super) fn update_fullscreen_viewport(&mut self) -> Result<(), B::Error> {
        let next_area = fullscreen_viewport(self.backend.size()?);
        if next_area == self.viewport_area {
            return Ok(());
        }
        self.set_viewport_area(next_area);
        self.invalidate_viewport();
        self.backend.clear_region(ClearType::All)?;
        Ok(())
    }

    pub(super) fn draw<F>(&mut self, render_callback: F) -> Result<(), B::Error>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.draw_with_previous(render_callback, false)
    }

    pub(super) fn draw_preserving_previous<F>(&mut self, render_callback: F) -> Result<(), B::Error>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.draw_with_previous(render_callback, true)
    }

    fn draw_with_previous<F>(
        &mut self,
        render_callback: F,
        preserve_previous: bool,
    ) -> Result<(), B::Error>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        let previous = 1 - self.current;
        if preserve_previous {
            self.buffers[self.current] = self.buffers[previous].clone();
            self.hyperlink_surfaces[self.current] = self.hyperlink_surfaces[previous].clone();
        } else {
            self.buffers[self.current].reset();
            self.hyperlink_surfaces[self.current].clear();
        }
        let cursor_position = {
            let buffer = &mut self.buffers[self.current];
            let hyperlinks = &mut self.hyperlink_surfaces[self.current];
            let mut frame = Frame {
                viewport_area: self.viewport_area,
                buffer,
                hyperlinks,
                cursor_position: None,
            };
            render_callback(&mut frame);
            frame.cursor_position
        };
        self.flush()?;
        if let Some(position) = cursor_position {
            self.backend.show_cursor()?;
            self.hidden_cursor = false;
            self.backend.set_cursor_position(position)?;
        } else {
            self.backend.hide_cursor()?;
            self.hidden_cursor = true;
        }
        Ok(())
    }

    pub(super) fn visible_text_snapshot(&self) -> VisibleTextSnapshot {
        VisibleTextSnapshot::from_buffer(&self.buffers[1 - self.current])
    }

    pub(super) fn clear(&mut self) -> Result<(), B::Error> {
        self.backend.set_cursor_position(Position::ORIGIN)?;
        self.backend.clear_region(ClearType::All)?;
        self.invalidate_viewport();
        Ok(())
    }

    fn flush(&mut self) -> Result<(), B::Error> {
        let previous = 1 - self.current;
        let updates = terminal_frame_diff(
            &self.buffers[previous],
            &self.buffers[self.current],
            &self.hyperlink_surfaces[previous],
            &self.hyperlink_surfaces[self.current],
            TerminalCompatibilityPolicy::current().supports_osc8_hyperlinks(),
        );
        self.backend
            .draw(updates.iter().map(|(x, y, cell)| (*x, *y, cell)))?;
        self.backend.flush()?;
        self.buffers[previous] = self.buffers[self.current].clone();
        self.hyperlink_surfaces[previous] = self.hyperlink_surfaces[self.current].clone();
        Ok(())
    }

    fn set_viewport_area(&mut self, area: Rect) {
        self.buffers[self.current].resize(area);
        self.buffers[1 - self.current].resize(area);
        self.viewport_area = area;
    }

    fn invalidate_viewport(&mut self) {
        self.buffers[1 - self.current] = Buffer::empty(self.viewport_area);
        self.hyperlink_surfaces[1 - self.current].clear();
    }
}

impl<B> Drop for Terminal<B>
where
    B: Backend,
{
    fn drop(&mut self) {
        if self.hidden_cursor {
            let _ = self.backend.show_cursor();
        }
    }
}

fn fullscreen_viewport(size: ratatui::layout::Size) -> Rect {
    Rect::new(0, 0, size.width, size.height)
}
