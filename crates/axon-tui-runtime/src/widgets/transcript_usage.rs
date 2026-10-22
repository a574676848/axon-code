//! Usage transcript cell 渲染。

use ratatui::text::Line;

use crate::surface::{
    cell::RenderCellData, cell_event_data::AgentRenderUsageCell, visual_density::NEUTRAL_PREFIX,
};
use crate::widgets::{transcript_cells::render_detail_block, transcript_style::CellVisual};

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    render_usage_lines(visual, data, width)
}

fn render_usage_lines(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(RenderCellData::Usage(cell)) = data else {
        return Vec::new();
    };
    if cell.total_tokens == 0 {
        return Vec::new();
    }
    render_detail_block(visual, NEUTRAL_PREFIX, &usage_summary(cell), width)
}

fn usage_summary(cell: &AgentRenderUsageCell) -> String {
    let mut parts = vec![format!("Token {}", cell.total)];
    push_nonzero_part(&mut parts, cell.input_tokens, "入", &cell.input);
    push_nonzero_part(
        &mut parts,
        cell.cache_read_input_tokens,
        "缓存命中",
        &cell.cache_read,
    );
    push_nonzero_part(
        &mut parts,
        cell.cache_write_input_tokens,
        "缓存写入",
        &cell.cache_write,
    );
    push_nonzero_part(&mut parts, cell.output_tokens, "出", &cell.output);
    push_nonzero_part(
        &mut parts,
        cell.reasoning_output_tokens,
        "推理",
        &cell.reasoning,
    );
    parts.join(" · ")
}

fn push_nonzero_part(parts: &mut Vec<String>, value: u64, label: &str, rendered: &str) {
    if value > 0 {
        parts.push(format!("{label} {rendered}"));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::render_standalone;
    use crate::widgets::transcript_style::CellVisual;
    use crate::{
        AgentTuiTokenUsage,
        surface::{cell::RenderCellData, cell_event_data::AgentRenderUsageCell},
    };

    #[test]
    fn usage_cell_uses_single_line_nonzero_parts() {
        let data = RenderCellData::Usage(AgentRenderUsageCell::from_usage(
            "usage:test:nonzero",
            AgentTuiTokenUsage::new(5_009, 0, 0, 104, 0),
        ));

        let text = render_standalone(test_visual(), false, Some(&data), 80)
            .into_iter()
            .flat_map(|line| line.spans.into_iter())
            .map(|span| span.content.into_owned())
            .collect::<String>();

        assert!(text.contains("Token 5.11K"));
        assert!(text.contains("入 5.01K"));
        assert!(text.contains("出 104"));
        assert!(!text.contains("缓存 0"));
        assert!(!text.contains("推理 0"));
        assert!(!text.contains("Usage"));
        assert!(!text.contains("total=5113"));
    }

    #[test]
    fn usage_with_zero_tokens_stays_quiet() {
        let data = RenderCellData::Usage(AgentRenderUsageCell::from_usage(
            "usage:test:zero",
            AgentTuiTokenUsage::new(0, 0, 0, 0, 0),
        ));

        assert!(render_standalone(test_visual(), false, Some(&data), 80).is_empty());
    }

    fn test_visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }
}
