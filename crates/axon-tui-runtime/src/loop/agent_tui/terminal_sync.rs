//! 终端同步更新边界。

use std::io::{self, Write};

use crossterm::{
    execute,
    terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use ratatui::backend::Backend;

use super::custom_terminal::Terminal;

impl<B> Terminal<B>
where
    B: Backend,
{
    pub(super) fn begin_synchronized_update(&mut self) -> io::Result<()>
    where
        B: Write,
    {
        execute!(self.backend_mut(), BeginSynchronizedUpdate)
    }

    pub(super) fn end_synchronized_update(&mut self) -> io::Result<()>
    where
        B: Write,
    {
        execute!(self.backend_mut(), EndSynchronizedUpdate)
    }
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, io};

    use ratatui::{
        backend::{Backend, TestBackend},
        buffer::Cell,
        layout::{Position, Size},
    };

    use super::Terminal;

    const ANSI_SYNC_BEGIN: &str = "\x1b[?2026h";
    const ANSI_SYNC_END: &str = "\x1b[?2026l";

    #[test]
    fn synchronized_update_writes_terminal_frame_boundaries() {
        let backend = CaptureBackend::new(20, 6);
        let mut terminal = Terminal::new_for_test(backend, 3).expect("测试终端应可创建");

        terminal
            .begin_synchronized_update()
            .expect("同步帧开始应可写入");
        terminal
            .end_synchronized_update()
            .expect("同步帧结束应可写入");

        let output = terminal.backend().output();
        let begin = output.find(ANSI_SYNC_BEGIN).expect("应写入同步帧开始标记");
        let end = output.find(ANSI_SYNC_END).expect("应写入同步帧结束标记");

        assert!(begin < end);
    }

    struct CaptureBackend {
        output: Vec<u8>,
        inner: TestBackend,
    }

    impl CaptureBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                output: Vec::new(),
                inner: TestBackend::new(width, height),
            }
        }

        fn output(&self) -> String {
            String::from_utf8_lossy(&self.output).into_owned()
        }
    }

    impl io::Write for CaptureBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.output.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Backend for CaptureBackend {
        type Error = Infallible;

        fn draw<'a, I>(&mut self, content: I) -> Result<(), Infallible>
        where
            I: Iterator<Item = (u16, u16, &'a Cell)>,
        {
            self.inner.draw(content)
        }

        fn hide_cursor(&mut self) -> Result<(), Infallible> {
            self.inner.hide_cursor()
        }

        fn show_cursor(&mut self) -> Result<(), Infallible> {
            self.inner.show_cursor()
        }

        fn get_cursor_position(&mut self) -> Result<Position, Infallible> {
            self.inner.get_cursor_position()
        }

        fn set_cursor_position<P: Into<Position>>(
            &mut self,
            position: P,
        ) -> Result<(), Infallible> {
            self.inner.set_cursor_position(position)
        }

        fn clear(&mut self) -> Result<(), Infallible> {
            self.inner.clear()
        }

        fn clear_region(
            &mut self,
            clear_type: ratatui::backend::ClearType,
        ) -> Result<(), Infallible> {
            self.inner.clear_region(clear_type)
        }

        fn append_lines(&mut self, line_count: u16) -> Result<(), Infallible> {
            self.inner.append_lines(line_count)
        }

        fn scroll_region_up(
            &mut self,
            region: std::ops::Range<u16>,
            scroll_by: u16,
        ) -> Result<(), Infallible> {
            self.inner.scroll_region_up(region, scroll_by)
        }

        fn scroll_region_down(
            &mut self,
            region: std::ops::Range<u16>,
            scroll_by: u16,
        ) -> Result<(), Infallible> {
            self.inner.scroll_region_down(region, scroll_by)
        }

        fn size(&self) -> Result<Size, Infallible> {
            self.inner.size()
        }

        fn window_size(&mut self) -> Result<ratatui::backend::WindowSize, Infallible> {
            self.inner.window_size()
        }

        fn flush(&mut self) -> Result<(), Infallible> {
            self.inner.flush()
        }
    }
}
