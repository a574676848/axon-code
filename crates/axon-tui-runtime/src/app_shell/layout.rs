//! AppShell 三段物理布局事实源。

use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub(crate) const HEADER_BAR_HEIGHT: u16 = 1;
pub(crate) const HEADER_BAR_MAX_HEIGHT: u16 = 2;
pub(crate) const SECTION_DIVIDER_HEIGHT: u16 = 1;
pub(crate) const IDLE_COMPOSER_HEIGHT: u16 = 3;
pub(crate) const AGENT_TUI_INITIAL_VIEWPORT_HEIGHT: u16 =
    HEADER_BAR_HEIGHT + SECTION_DIVIDER_HEIGHT + IDLE_COMPOSER_HEIGHT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppShellAreas {
    pub(crate) header: Rect,
    pub(crate) viewport: Rect,
    pub(crate) composer: Rect,
}

pub(crate) fn layout_app_shell(
    area: Rect,
    composer_height: u16,
    header_height: u16,
) -> AppShellAreas {
    let header_height = header_height.clamp(HEADER_BAR_HEIGHT, HEADER_BAR_MAX_HEIGHT);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_height),
            Constraint::Length(SECTION_DIVIDER_HEIGHT),
            Constraint::Min(0),
            Constraint::Length(composer_height),
        ])
        .split(area);

    AppShellAreas {
        header: chunks[0],
        viewport: chunks[2],
        composer: chunks[3],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_shell_keeps_header_top_and_composer_bottom() {
        let area = Rect::new(0, 0, 80, 24);
        let layout = layout_app_shell(area, 3, HEADER_BAR_MAX_HEIGHT);

        assert_eq!(layout.header.y, area.y);
        assert_eq!(layout.header.height, HEADER_BAR_MAX_HEIGHT);
        assert_eq!(
            layout.viewport.y,
            layout.header.bottom() + SECTION_DIVIDER_HEIGHT
        );
        assert_eq!(layout.viewport.bottom(), layout.composer.y);
        assert_eq!(layout.composer.y + layout.composer.height, area.bottom());
    }
}
