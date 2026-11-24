//! Turn summary transcript cell 渲染。

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{
    r#loop::agent_tui::{AgentTuiTokenUsage, duration_format::format_duration_millis_compact},
    surface::{cell::RenderCellData, display_width::DisplayWidthText},
    widgets::transcript_style::CellVisual,
};

const DIVIDER: &str = "─";
const LABEL_PREFIX: &str = "─ ";
const LABEL_SUFFIX: &str = " ─";

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(RenderCellData::TurnSummary(cell)) = data else {
        return Vec::new();
    };
    let label = turn_summary_label(
        cell.duration_ms,
        cell.total_tokens,
        cell.usage,
        cell.first_token_avg_ms,
        cell.generation_tps_x10,
    );
    vec![divider_line(
        label.as_deref(),
        usize::from(width),
        visual.detail_text_style,
    )]
}

fn turn_summary_label(
    duration_ms: Option<u64>,
    total_tokens: Option<u64>,
    usage: Option<AgentTuiTokenUsage>,
    first_token_avg_ms: Option<u64>,
    generation_tps_x10: Option<u64>,
) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(duration_ms) = duration_ms.filter(|value| *value > 0) {
        parts.push(format!(
            "用时 {}",
            format_duration_millis_compact(duration_ms)
        ));
    }
    if let Some(usage) = usage {
        parts.push(format!(
            "Token {}",
            AgentTuiTokenUsage::render_token_count(usage.total_tokens())
        ));
        parts.push(format!(
            "入 {}",
            AgentTuiTokenUsage::render_token_count(usage.input_tokens)
        ));
        parts.push(format!(
            "缓存命中 {}",
            AgentTuiTokenUsage::render_token_count(usage.cache_read_input_tokens)
        ));
        parts.push(format!(
            "出 {}",
            AgentTuiTokenUsage::render_token_count(usage.output_tokens)
        ));
        parts.push(format!(
            "推理 {}",
            AgentTuiTokenUsage::render_token_count(usage.reasoning_output_tokens)
        ));
        parts.push(format!(
            "命中率 {}",
            axon_display::format_basis_points_percent(usage.cache_hit_rate_basis_points())
        ));
    } else if let Some(total_tokens) = total_tokens.filter(|value| *value > 0) {
        parts.push(format!(
            "Token {}",
            AgentTuiTokenUsage::render_token_count(total_tokens)
        ));
    }
    if let Some(first_token_avg_ms) = first_token_avg_ms {
        parts.push(format!(
            "首字 {}",
            format_duration_millis_compact(first_token_avg_ms)
        ));
    }
    if let Some(generation_tps_x10) = generation_tps_x10 {
        parts.push(format!(
            "速率 {}",
            format_tokens_per_second_x10(generation_tps_x10)
        ));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn format_tokens_per_second_x10(value: u64) -> String {
    if value.is_multiple_of(10) {
        format!("{} tok/s", value / 10)
    } else {
        format!("{}.{value_mod} tok/s", value / 10, value_mod = value % 10)
    }
}

fn divider_line(label: Option<&str>, width: usize, style: Style) -> Line<'static> {
    let width = width.max(1);
    let Some(label) = label else {
        return Line::from(Span::styled(DIVIDER.repeat(width), style));
    };
    let text = format!("{LABEL_PREFIX}{label}{LABEL_SUFFIX}");
    let text = DisplayWidthText::ellipsize(&text, width);
    let text_width = DisplayWidthText::width(&text);
    let remaining_width = width.saturating_sub(text_width);
    let prefix_width = remaining_width / 2;
    let suffix_width = remaining_width.saturating_sub(prefix_width);
    Line::from(vec![
        Span::styled(DIVIDER.repeat(prefix_width), style),
        Span::styled(text, style),
        Span::styled(DIVIDER.repeat(suffix_width), style),
    ])
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::{divider_line, render_standalone};
    use crate::{
        surface::{cell::RenderCellData, cell_turn_summary_data::AgentRenderTurnSummaryCell},
        widgets::transcript_style::CellVisual,
    };

    #[test]
    fn turn_summary_uses_project_duration_and_token_units() {
        let lines = render_standalone(
            visual(),
            false,
            Some(&RenderCellData::TurnSummary(
                AgentRenderTurnSummaryCell::new(
                    "turn-summary:test",
                    Some(65_000),
                    Some(crate::AgentTuiTokenUsage::new(5_113, 9_700, 300, 104, 38)),
                )
                .with_performance(Some(8_000), Some(400)),
            )),
            160,
        );

        let text = lines[0]
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(text.contains("用时 1m 05s"));
        assert!(text.contains("Token 15.3K"));
        assert!(text.contains("入 5.11K"));
        assert!(text.contains("缓存命中 9.7K"));
        assert!(text.contains("出 104"));
        assert!(text.contains("推理 38"));
        assert!(text.contains("命中率 64.18%"));
        assert!(text.contains("首字 8s"));
        assert!(text.contains("速率 40 tok/s"));
        assert!(!text.contains("65000ms"));
        assert!(!text.contains("5113"));
        assert!(!text.contains("缓存写入"));
    }

    #[test]
    fn turn_summary_divider_centers_label() {
        let text = line_text(&divider_line(Some("用时 5m 00s"), 80, Style::default()));
        let label_start = text.find("用时").expect("应渲染用时标签");
        let label_end = label_start + "用时 5m 00s".len();
        let left_width =
            crate::surface::display_width::DisplayWidthText::width(&text[..label_start]);
        let right_width =
            crate::surface::display_width::DisplayWidthText::width(&text[label_end..]);

        assert!(
            left_width.abs_diff(right_width) <= 1,
            "用时标签应居中，left={left_width}, right={right_width}, line={text}"
        );
    }

    #[test]
    fn turn_summary_without_generation_metrics_stays_compatible() {
        let lines = render_standalone(
            visual(),
            false,
            Some(&RenderCellData::TurnSummary(
                AgentRenderTurnSummaryCell::new(
                    "turn-summary:legacy",
                    Some(65_000),
                    Some(crate::AgentTuiTokenUsage::new(5_113, 9_700, 300, 104, 38)),
                ),
            )),
            160,
        );
        let text = line_text(&lines[0]);

        assert!(text.contains("Token 15.3K"));
        assert!(!text.contains("首字"));
        assert!(!text.contains("速率"));
    }

    fn line_text(line: &ratatui::text::Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    fn visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }
}
