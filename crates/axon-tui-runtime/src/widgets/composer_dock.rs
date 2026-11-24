//! ComposerDock - 底部输入与即时动作区。

use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

use crate::{app_shell::composer_dock::ComposerDockViewModel, widgets::DockRenderGateway};

pub(crate) struct ComposerDock<'a> {
    pub(crate) view_model: ComposerDockViewModel<'a>,
}

impl Widget for ComposerDock<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        DockRenderGateway::render(self.view_model, area, buf);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::*;
    use crate::app_shell::composer_dock::{
        ComposerDockDraft, ComposerDockViewModel, WorkingStatusViewModel,
    };
    use crate::app_shell::style_tokens::{TEXT_SECONDARY, TranscriptColorRole, transcript_palette};
    use crate::surface::subagent_activity::{
        SubagentActivityRow, SubagentActivitySnapshot, SubagentActivityStatus,
    };
    use crate::widgets::dock_render_gateway::activity_divider_text;

    #[test]
    fn composer_dock_working_status_keeps_top_spacing_with_low_emphasis_style() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 5));

        ComposerDock {
            view_model: ComposerDockViewModel {
                draft: ComposerDockDraft {
                    text: "继续输入",
                    elements: Vec::new(),
                },
                command_suggestions: &[],
                popup_items: &[],
                suggestion_title: "",
                selected_command_suggestion: 0,
                queued_preview: None,
                working_status: Some(WorkingStatusViewModel::generating(
                    "cpa/gpt-5.5".to_owned(),
                    Some(std::time::Duration::from_secs(204)),
                    false,
                )),
                draft_scroll_offset: 0,
                panel: None,
                subagent_activity: SubagentActivitySnapshot::default(),
            },
        }
        .render(buffer.area, &mut buffer);

        assert!(row_text(&buffer, 0).trim().is_empty());
        let row = row_text(&buffer, 1);
        let normalized = normalized_text(&row);
        assert!(normalized.starts_with("◌cpa/gpt-5.5生成回复中"));
        assert!(normalized.contains("（3m24s·按ESC中断）"));

        let palette = transcript_palette();
        assert_eq!(
            find_cell_in_row(&buffer, 1, "◌").fg,
            palette.color(TranscriptColorRole::Muted)
        );
        assert_eq!(
            find_cell_in_row(&buffer, 1, "（").fg,
            palette.color(TranscriptColorRole::Muted)
        );
        assert!(
            working_primary_has_shimmer_style(&buffer, 1, &normalized),
            "working primary label 应使用低强调扫光动画"
        );
    }

    #[test]
    fn composer_dock_renders_subagent_activity_below_input_with_project_units() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 100, 8));

        ComposerDock {
            view_model: ComposerDockViewModel {
                draft: ComposerDockDraft {
                    text: "继续输入",
                    elements: Vec::new(),
                },
                command_suggestions: &[],
                popup_items: &[],
                suggestion_title: "",
                selected_command_suggestion: 0,
                queued_preview: None,
                working_status: None,
                draft_scroll_offset: 0,
                panel: None,
                subagent_activity: SubagentActivitySnapshot {
                    rows: vec![
                        SubagentActivityRow {
                            agent_id: "subagent:/root/review".to_owned(),
                            label: "/root/review".to_owned(),
                            status: SubagentActivityStatus::Running,
                            activity: "执行工具".to_owned(),
                            elapsed: std::time::Duration::from_secs(65),
                            usage: Some(12_345),
                            cache_hit_rate_basis_points: Some(6_000),
                            model: Some("openai/gpt-test".to_owned()),
                            summary: Some("正在检查接口契约".to_owned()),
                        },
                        SubagentActivityRow {
                            agent_id: "subagent:/root/analysis_1".to_owned(),
                            label: "/root/analysis_1".to_owned(),
                            status: SubagentActivityStatus::Running,
                            activity: "生成中".to_owned(),
                            elapsed: std::time::Duration::from_millis(1080),
                            usage: None,
                            cache_hit_rate_basis_points: None,
                            model: None,
                            summary: None,
                        },
                        SubagentActivityRow {
                            agent_id: "subagent:/root/delegate_fast_2".to_owned(),
                            label: "/root/delegate_fast_2".to_owned(),
                            status: SubagentActivityStatus::Running,
                            activity: "运行中".to_owned(),
                            elapsed: std::time::Duration::from_secs(2),
                            usage: None,
                            cache_hit_rate_basis_points: None,
                            model: None,
                            summary: None,
                        },
                    ],
                    max_visible_rows: 4,
                    animation_frame: 0,
                },
            },
        }
        .render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(text.contains("继续输入"));
        assert!(text.contains("子代理活动"));
        assert!(text.contains("子代理/root/review"));
        assert!(text.contains("openai/gpt-test"));
        assert!(text.contains("执行工具"));
        assert!(text.contains("token12.3K"));
        assert!(text.contains("命中60.00%"));
        assert!(text.contains("·1m"));
        assert!(text.contains("子代理/root/analysis_1·生成中"));
        assert!(text.contains("子代理/root/delegate_fast_2·运行中"));
        assert!(!text.contains("token待回传"));
        assert!(!text.contains("token未回传"));
        assert!(!text.contains("token0"));
        assert!(!text.contains("正在检查接口契约"));
        let active_color = transcript_palette().color(TranscriptColorRole::SubagentActive);
        assert!(
            buffer
                .content()
                .iter()
                .any(|cell| { cell.symbol() == "子" && cell.fg == active_color })
        );
    }

    #[test]
    fn subagent_activity_divider_falls_back_to_line_when_narrow() {
        assert_eq!(activity_divider_text(4), "────");
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }

    fn normalized_text(text: &str) -> String {
        text.split_whitespace().collect::<String>()
    }

    fn row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    fn find_cell_in_row<'a>(buffer: &'a Buffer, y: u16, symbol: &str) -> &'a ratatui::buffer::Cell {
        (0..buffer.area.width)
            .map(|x| &buffer[(x, y)])
            .find(|cell| cell.symbol() == symbol)
            .expect("目标文本应渲染到指定行")
    }

    fn working_primary_has_shimmer_style(buffer: &Buffer, y: u16, normalized: &str) -> bool {
        let primary = "cpa/gpt-5.5生成回复中";
        if !normalized.contains(primary) {
            return false;
        }
        let Some(start) = row_cell_x(buffer, y, "c") else {
            return false;
        };
        let Some(end) = row_cell_x(buffer, y, "（") else {
            return false;
        };
        let muted_color = transcript_palette().color(TranscriptColorRole::Muted);
        let cells = (start..end)
            .map(|x| &buffer[(x, y)])
            .filter(|cell| cell.fg != ratatui::style::Color::Reset)
            .collect::<Vec<_>>();
        !cells.is_empty()
            && cells.iter().all(|cell| {
                let color_is_low_emphasis = match (muted_color, cell.fg, TEXT_SECONDARY) {
                    (
                        ratatui::style::Color::Rgb(mr, mg, mb),
                        ratatui::style::Color::Rgb(r, g, b),
                        ratatui::style::Color::Rgb(sr, sg, sb),
                    ) => (mr..=sr).contains(&r) && (mg..=sg).contains(&g) && (mb..=sb).contains(&b),
                    _ => cell.fg == muted_color,
                };
                color_is_low_emphasis && cell.modifier.is_empty()
            })
    }

    fn row_cell_x(buffer: &Buffer, y: u16, symbol: &str) -> Option<u16> {
        (0..buffer.area.width).find(|&x| buffer[(x, y)].symbol() == symbol)
    }
}
