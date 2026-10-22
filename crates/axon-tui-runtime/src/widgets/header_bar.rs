//! HeaderBar - 顶部会话状态栏。

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::{
    app_shell::{
        header_bar::{
            HeaderBarPriority, HeaderBarSegment, HeaderBarSegmentKind, HeaderBarSegmentTone,
            HeaderViewModel, primary_separator,
        },
        style_tokens::{ACCENT_GREEN, ACCENT_MINT, ACCENT_RED, TEXT_MUTED, TEXT_SECONDARY},
    },
    surface::display_width::DisplayWidthText,
    widgets::transcript_style::syntax_role_style,
};

const SEPARATOR_STYLE: Style = Style::new().fg(TEXT_MUTED);
const STATUS_STYLE: Style = Style::new().fg(ACCENT_MINT).add_modifier(Modifier::BOLD);
const STATUS_BREATH_STYLE: Style = Style::new().fg(ACCENT_GREEN).add_modifier(Modifier::BOLD);
const CONTEXT_STYLE: Style = Style::new().fg(TEXT_SECONDARY);
const CONTEXT_WARNING_STYLE: Style = Style::new().fg(TEXT_SECONDARY);
const CONTEXT_CRITICAL_STYLE: Style = Style::new().fg(ACCENT_RED).add_modifier(Modifier::BOLD);
const HINT_STYLE: Style = Style::new().fg(TEXT_MUTED);
const CONTINUATION_PREFIX: &str = "· ";

pub(crate) struct HeaderBar<'a> {
    pub(crate) view_model: HeaderViewModel<'a>,
}

impl HeaderBar<'_> {
    pub(crate) fn required_height(&self, width: u16) -> u16 {
        let (_, second) = self.segment_rows(width);
        if second.is_empty() { 1 } else { 2 }
    }

    fn build_lines(&self, width: u16) -> (Line<'static>, Line<'static>) {
        let (first, second) = self.segment_rows(width);
        (
            joined_line(first, primary_separator()),
            joined_line_with_prefix(second, primary_separator(), CONTINUATION_PREFIX),
        )
    }

    fn segment_rows(&self, width: u16) -> (Vec<HeaderBarSegment>, Vec<HeaderBarSegment>) {
        let product = self.view_model.product_view_model();
        fit_segment_rows(&product.primary, &product.hints, primary_separator(), width)
    }
}

impl Widget for HeaderBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let (first, second) = self.build_lines(area.width);
        buf.set_line(area.x, area.y, &first, area.width);
        if area.height > 1 && !second.spans.is_empty() {
            buf.set_line(area.x, area.y.saturating_add(1), &second, area.width);
        }
    }
}

fn fit_segment_rows(
    primary: &[HeaderBarSegment],
    hints: &[HeaderBarSegment],
    separator: &str,
    width: u16,
) -> (Vec<HeaderBarSegment>, Vec<HeaderBarSegment>) {
    let target_width = usize::from(width);
    let continuation_width = DisplayWidthText::width(CONTINUATION_PREFIX);
    let second_target_width = target_width.saturating_sub(continuation_width);
    let mut first = Vec::new();
    let mut second = Vec::new();
    let mut first_row_closed = false;
    for priority in [
        HeaderBarPriority::P0,
        HeaderBarPriority::P1,
        HeaderBarPriority::P2,
        HeaderBarPriority::P3,
    ] {
        for segment in primary
            .iter()
            .filter(|segment| segment.priority == priority)
        {
            if priority == HeaderBarPriority::P0
                || (!first_row_closed && row_accepts(&first, segment, separator, target_width))
            {
                first.push(segment.clone());
            } else {
                first_row_closed = true;
                if priority != HeaderBarPriority::P0
                    && row_accepts(&second, segment, separator, second_target_width)
                {
                    second.push(segment.clone());
                }
            }
        }
    }
    for segment in hints {
        if row_accepts(&second, segment, separator, second_target_width) {
            second.push(segment.clone());
        }
    }
    (first, second)
}

fn row_accepts(
    row: &[HeaderBarSegment],
    segment: &HeaderBarSegment,
    separator: &str,
    target_width: usize,
) -> bool {
    if row.is_empty() {
        return true;
    }
    let mut proposed = row.to_vec();
    proposed.push(segment.clone());
    line_width(&proposed, separator) <= target_width
}

fn joined_line(segments: Vec<HeaderBarSegment>, separator: &str) -> Line<'static> {
    joined_line_with_prefix(segments, separator, "")
}

fn joined_line_with_prefix(
    segments: Vec<HeaderBarSegment>,
    separator: &str,
    prefix: &str,
) -> Line<'static> {
    let mut spans = Vec::new();
    if !segments.is_empty() && !prefix.is_empty() {
        spans.push(Span::styled(prefix.to_owned(), SEPARATOR_STYLE));
    }
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(separator.to_owned(), SEPARATOR_STYLE));
        }
        spans.push(Span::styled(segment.text.clone(), segment_style(segment)));
    }
    Line::from(spans)
}

fn line_width(segments: &[HeaderBarSegment], separator: &str) -> usize {
    let content = segments
        .iter()
        .map(|segment| DisplayWidthText::width(segment.text.as_str()))
        .sum::<usize>();
    let separators = segments.len().saturating_sub(1) * DisplayWidthText::width(separator);
    content + separators
}

fn segment_style(segment: &HeaderBarSegment) -> Style {
    match (segment.kind, segment.tone) {
        (HeaderBarSegmentKind::Context, HeaderBarSegmentTone::Warning) => CONTEXT_WARNING_STYLE,
        (HeaderBarSegmentKind::Context, HeaderBarSegmentTone::Critical) => CONTEXT_CRITICAL_STYLE,
        (HeaderBarSegmentKind::Status, _) if segment.breath_highlight => STATUS_BREATH_STYLE,
        (HeaderBarSegmentKind::Status, _) => STATUS_STYLE,
        (HeaderBarSegmentKind::Model, _) => syntax_role_style(
            &["entity.name.type", "support.type", "variable"],
            crate::app_shell::style_tokens::TranscriptColorRole::Muted,
        )
        .add_modifier(Modifier::BOLD),
        (HeaderBarSegmentKind::Context, _) => CONTEXT_STYLE,
        (HeaderBarSegmentKind::Lsp, _) => syntax_role_style(
            &["markup.inserted", "constant.numeric"],
            crate::app_shell::style_tokens::TranscriptColorRole::ToolCompleted,
        )
        .add_modifier(Modifier::BOLD),
        (
            HeaderBarSegmentKind::Cwd
            | HeaderBarSegmentKind::ReplaySnapshot
            | HeaderBarSegmentKind::ReplayInput
            | HeaderBarSegmentKind::Elapsed
            | HeaderBarSegmentKind::FirstToken
            | HeaderBarSegmentKind::GenerationRate
            | HeaderBarSegmentKind::Plan,
            _,
        ) => HINT_STYLE,
        (HeaderBarSegmentKind::Reasoning, _) => syntax_role_style(
            &["entity.name.type", "support.type", "variable"],
            crate::app_shell::style_tokens::TranscriptColorRole::Muted,
        ),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    use super::*;
    use crate::app_shell::header_bar::{HeaderBarMetrics, HeaderBarStatus};
    use crate::r#loop::agent_tui::{
        AgentTuiConfigStatus, AgentTuiPlanProgress, AgentTuiRunOverrides, AgentTuiTokenUsage,
    };

    #[test]
    fn header_bar_renders_product_status_and_hints_in_two_rows() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 120, 2));
        let header = HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics {
                    cwd: "E:/work".to_owned(),
                    status: HeaderBarStatus::Working,
                    spinner: "⚡ Axon",
                    token_usage: AgentTuiTokenUsage::new(1_000, 2_000, 0, 0, 0)
                        .with_current_context_tokens(2_000),
                    ..HeaderBarMetrics::default()
                },
                models: &[],
            },
        };

        header.render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(text.contains("⚡Axon"));
        assert!(!text.contains("生成回复中"));
        assert!(text.contains("模型gpt-4o"));
        assert!(text.contains("上下文2K"));
        assert!(!text.contains("Esc中断"));
        let hint_row = normalized_text(&buffer_row_text(&buffer, 1));
        assert!(!hint_row.contains("Esc中断"));
    }

    #[test]
    fn header_bar_status_logo_uses_breathing_style() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 2));
        let header = HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics {
                    status: HeaderBarStatus::Working,
                    spinner: "⚡ Axon",
                    animation_tick: 0,
                    cwd: "E:/work".to_owned(),
                    ..HeaderBarMetrics::default()
                },
                models: &[],
            },
        };

        header.render(buffer.area, &mut buffer);

        assert_eq!(Some(buffer[(0, 0)].fg), STATUS_BREATH_STYLE.fg);
    }

    #[test]
    fn header_model_uses_syntax_theme_style() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-5.6", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 80, 1));
        HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics::default(),
                models: &[],
            },
        }
        .render(buffer.area, &mut buffer);

        let model_cell = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "模")
            .expect("Header 应渲染模型段");
        let expected = syntax_role_style(
            &["entity.name.type", "support.type", "variable"],
            crate::app_shell::style_tokens::TranscriptColorRole::Muted,
        );
        assert_eq!(model_cell.fg, expected.fg.unwrap_or(TEXT_SECONDARY));
        assert!(model_cell.modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn context_pressure_tone_changes_header_style_at_critical_threshold() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-5", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let models = [crate::AgentTuiModelCatalogItem::new_with_context_window(
            "gpt-5",
            "gpt-5",
            "openai",
            Some(1_000),
        )];
        let header = HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics {
                    token_usage: AgentTuiTokenUsage::default().with_current_context_tokens(850),
                    ..HeaderBarMetrics::default()
                },
                models: &models,
            },
        };

        let product = header.view_model.product_view_model();
        let context = product
            .primary
            .iter()
            .find(|segment| segment.kind == HeaderBarSegmentKind::Context)
            .expect("HeaderBar 应包含上下文段");

        assert_eq!(context.tone, HeaderBarSegmentTone::Critical);
        assert_eq!(segment_style(context), CONTEXT_CRITICAL_STYLE);
    }

    #[test]
    fn header_bar_keeps_p0_fields_when_width_is_tight() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 18, 2));
        let header = HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics {
                    cwd: "E:/very/long/workspace/path/axon-code".to_owned(),
                    status: HeaderBarStatus::Ready,
                    session_id: Some("019eead4-7469-7130-ad0b-47d4ec1a87a4".to_owned()),
                    token_usage: AgentTuiTokenUsage::new(0, 0, 0, 0, 0)
                        .with_current_context_tokens(9_999),
                    ..HeaderBarMetrics::default()
                },
                models: &[],
            },
        };

        header.render(buffer.area, &mut buffer);

        let text = normalized_text(&buffer_text(&buffer));
        assert!(!text.contains("就绪"));
        assert!(text.contains("模型gpt-4o"));
        assert!(!text.contains("E:/very/long"));
        assert!(!text.contains("上下文 "));
        assert!(!text.contains("会话:"));
    }

    #[test]
    fn header_bar_continuation_row_starts_with_separator_prefix() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let mut buffer = Buffer::empty(Rect::new(0, 0, 30, 2));
        let header = HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics {
                    cwd: "E:/work".to_owned(),
                    first_token_avg_ms: Some(8_000),
                    generation_tps_x10: Some(400),
                    ..HeaderBarMetrics::default()
                },
                models: &[],
            },
        };

        header.render(buffer.area, &mut buffer);

        let second_row = buffer_row_text(&buffer, 1);
        assert!(second_row.starts_with(CONTINUATION_PREFIX));
        assert!(
            second_row[CONTINUATION_PREFIX.len()..]
                .chars()
                .any(|character| !character.is_whitespace())
        );
    }

    #[test]
    fn header_bar_keeps_plan_after_rate_when_rate_wraps() {
        let config_status = AgentTuiConfigStatus::new("openai", "gpt-4o", "已认证");
        let overrides = AgentTuiRunOverrides::default();
        let header = HeaderBar {
            view_model: HeaderViewModel {
                config_status: &config_status,
                overrides: &overrides,
                metrics: HeaderBarMetrics {
                    cwd: "E:/work".to_owned(),
                    first_token_avg_ms: Some(5_530),
                    generation_tps_x10: Some(15_336),
                    plan_progress: Some(AgentTuiPlanProgress {
                        completed: 1,
                        total: 3,
                    }),
                    ..HeaderBarMetrics::default()
                },
                models: &[],
            },
        };

        let product = header.view_model.product_view_model();
        let (first, second) =
            fit_segment_rows(&product.primary, &product.hints, primary_separator(), 50);

        let ordered_metrics = first
            .iter()
            .chain(second.iter())
            .filter(|segment| {
                matches!(
                    segment.kind,
                    HeaderBarSegmentKind::FirstToken
                        | HeaderBarSegmentKind::GenerationRate
                        | HeaderBarSegmentKind::Plan
                )
            })
            .map(|segment| segment.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            ordered_metrics,
            [
                HeaderBarSegmentKind::FirstToken,
                HeaderBarSegmentKind::GenerationRate,
                HeaderBarSegmentKind::Plan,
            ]
        );
        assert!(second.iter().any(|segment| segment.text.contains("速率 ")));
        assert!(second.iter().any(|segment| segment.text.contains("计划 ")));
    }

    fn buffer_text(buffer: &Buffer) -> String {
        buffer
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>()
    }

    fn buffer_row_text(buffer: &Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    }

    fn normalized_text(text: &str) -> String {
        text.split_whitespace().collect::<String>()
    }
}
