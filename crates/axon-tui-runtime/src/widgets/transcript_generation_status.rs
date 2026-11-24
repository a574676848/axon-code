//! Generation status transcript cell 渲染。

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::{
        AgentTuiLifecyclePhase,
        display_units::{format_duration_millis_compact, normalize_display_units_in_text},
    },
    surface::{
        cell::{RenderCellData, RenderLifecycleItem},
        visual_density::{BULLET_PREFIX, STATUS_RUNNING_PREFIX},
    },
    widgets::{
        shimmer::shimmer_spans,
        transcript_cells::{
            CELL_DETAIL_LIMIT, normalize_and_truncate_cell_text, render_wrapped_block,
        },
        transcript_style::{CellVisual, role_style, strong_role_style},
    },
};

const GENERATION_STATUS_PREFIX: &str = STATUS_RUNNING_PREFIX;
const COMMAND_PREFIX: &str = "  › ";
const DETAIL_PREFIX: &str = BULLET_PREFIX;

pub(super) fn render_standalone(
    visual: CellVisual,
    selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(RenderCellData::GenerationStatus(cell)) = data else {
        return Vec::new();
    };
    let mut lines = render_current_item(visual, &cell.items, width);
    if selected {
        reverse_first_line(&mut lines);
    }
    lines
}

fn render_current_item(
    visual: CellVisual,
    items: &[RenderLifecycleItem],
    width: u16,
) -> Vec<Line<'static>> {
    items
        .iter()
        .rev()
        .find(|item| !generation_item_text(item).is_empty())
        .map_or_else(Vec::new, |item| render_item(visual, item, width))
}

fn render_item(visual: CellVisual, item: &RenderLifecycleItem, width: u16) -> Vec<Line<'static>> {
    let text = normalize_and_truncate_cell_text(&generation_item_text(item), CELL_DETAIL_LIMIT);
    if text.is_empty() {
        return Vec::new();
    }
    if is_streaming_phase(item.phase) {
        return render_streaming_item(visual, item, &text);
    }
    let prefix = prefix_for_item(item);
    let prefix_style = prefix_style_for_item(item, visual);
    render_wrapped_block(
        &prefix,
        &" ".repeat(prefix.len()),
        &text,
        prefix_style,
        visual.detail_text_style,
        width,
    )
}

fn is_streaming_phase(phase: AgentTuiLifecyclePhase) -> bool {
    matches!(
        phase,
        AgentTuiLifecyclePhase::Streaming | AgentTuiLifecyclePhase::Generation
    )
}

fn render_streaming_item(
    visual: CellVisual,
    item: &RenderLifecycleItem,
    text: &str,
) -> Vec<Line<'static>> {
    let prefix = prefix_for_item(item);
    let prefix_style = prefix_style_for_item(item, visual);
    let base_color = prefix_style.fg.unwrap_or_else(|| {
        crate::app_shell::style_tokens::transcript_palette().color(TranscriptColorRole::Muted)
    });
    let mut prefix_spans: Vec<Span<'static>> = vec![Span::styled(prefix.clone(), prefix_style)];
    prefix_spans.extend(shimmer_spans(text, base_color));
    vec![Line::from(prefix_spans)]
}

fn prefix_for_item(item: &RenderLifecycleItem) -> String {
    item.indicator
        .as_deref()
        .filter(|indicator| !indicator.trim().is_empty())
        .map_or_else(
            || default_prefix_for_item(item).to_owned(),
            |indicator| format!("  {indicator} "),
        )
}

fn default_prefix_for_item(item: &RenderLifecycleItem) -> &'static str {
    match item.phase {
        AgentTuiLifecyclePhase::Streaming | AgentTuiLifecyclePhase::Generation => {
            GENERATION_STATUS_PREFIX
        }
        AgentTuiLifecyclePhase::Command => COMMAND_PREFIX,
        AgentTuiLifecyclePhase::Notice | AgentTuiLifecyclePhase::Unknown => DETAIL_PREFIX,
    }
}

fn prefix_style_for_item(item: &RenderLifecycleItem, visual: CellVisual) -> Style {
    match item.phase {
        AgentTuiLifecyclePhase::Streaming => strong_role_style(TranscriptColorRole::Accent),
        AgentTuiLifecyclePhase::Generation => role_style(TranscriptColorRole::Loading),
        AgentTuiLifecyclePhase::Command => role_style(TranscriptColorRole::DiffHunk),
        AgentTuiLifecyclePhase::Notice => role_style(TranscriptColorRole::Notice),
        AgentTuiLifecyclePhase::Unknown => visual.detail_prefix_style,
    }
}

fn generation_item_text(item: &RenderLifecycleItem) -> String {
    let mut parts = Vec::new();
    if let Some(tool) = retry_model_name(item) {
        parts.push(tool);
    }
    if let Some(attempt) = retry_attempt_index(item) {
        parts.push(format!("尝试{attempt}"));
    }
    let summary = summary_text(item);
    if !summary.is_empty() {
        parts.push(summary);
    }
    if streaming_started_item(item)
        && let Some(duration) = item.duration_ms
    {
        parts.push(format!("首字 {}", format_duration_millis_compact(duration)));
    }
    parts.join(" · ")
}

fn streaming_started_item(item: &RenderLifecycleItem) -> bool {
    matches!(item.kind.as_str(), "StreamingStarted" | "streaming_started")
}

fn summary_text(item: &RenderLifecycleItem) -> String {
    let summary = item.summary.trim();
    if summary.is_empty()
        || summary_duplicates_context(item, summary)
        || streaming_reply_status(item, summary)
    {
        return String::new();
    }
    normalize_display_units_in_text(summary)
}

fn streaming_reply_status(item: &RenderLifecycleItem, summary: &str) -> bool {
    matches!(
        item.kind.as_str(),
        "StreamingStarted" | "streaming_started" | "Generation" | "generation"
    ) && (summary == "生成回复中" || summary.ends_with(" 生成回复中"))
}

fn summary_duplicates_context(item: &RenderLifecycleItem, summary: &str) -> bool {
    if item
        .tool_name
        .as_deref()
        .is_some_and(|tool| summary == tool || summary == format!("运行 {tool}"))
    {
        return true;
    }
    matches!(
        (item.kind.as_str(), summary),
        ("StreamingStarted" | "streaming_started", "运行")
            | ("Generation" | "generation", "生成")
            | ("ProviderRetry" | "provider_retry", _)
    )
}

fn reverse_first_line(lines: &mut [Line<'static>]) {
    if let Some(first) = lines.first_mut() {
        first.spans.iter_mut().for_each(|span| {
            span.style = span.style.add_modifier(Modifier::REVERSED);
        });
    }
}

fn retry_attempt_index(item: &RenderLifecycleItem) -> Option<usize> {
    matches!(item.kind.as_str(), "ProviderRetry" | "provider_retry").then_some(item.attempt_index?)
}

fn retry_model_name(item: &RenderLifecycleItem) -> Option<String> {
    if !matches!(item.kind.as_str(), "ProviderRetry" | "provider_retry") {
        return None;
    }
    item.tool_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(str::trim)
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Style, text::Line};

    use super::render_standalone;
    use crate::widgets::transcript_style::CellVisual;
    use crate::{
        AgentTuiLifecyclePhase,
        surface::cell::{RenderCellData, RenderGenerationStatusCell, RenderLifecycleItem},
    };

    #[test]
    fn generation_status_uses_latest_running_item_as_primary() {
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:primary".to_owned(),
            title: "生成".to_owned(),
            items: vec![
                item(
                    "StreamingStarted",
                    "local/model",
                    AgentTuiLifecyclePhase::Streaming,
                ),
                item("Generation", "生成回复", AgentTuiLifecyclePhase::Generation),
            ],
        });

        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.starts_with("  ◌ 生成回复"));
        assert!(!text.contains("local/model"));
        assert!(!text.contains("生成 · 生成回复"));
    }

    #[test]
    fn generation_status_uses_structured_metadata_without_raw_milliseconds() {
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:retry".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:retry:item".to_owned(),
                kind: "ProviderRetry".to_owned(),
                summary: "Provider 重试 1/2：瞬时传输错误".to_owned(),
                phase: AgentTuiLifecyclePhase::Streaming,
                indicator: Some("⠋".to_owned()),
                tool_name: Some("cpa/gpt-5.4".to_owned()),
                duration_ms: Some(65_000),
                attempt_index: Some(2),
            }],
        });

        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.contains("cpa/gpt-5.4"));
        assert!(text.contains("尝试2"));
        assert!(!text.contains("Provider 重试"));
        assert!(!text.contains("1m 05s"));
        assert!(!text.contains("65000ms"));
    }

    #[test]
    fn generation_status_summary_normalizes_raw_millisecond_units() {
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:milliseconds".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:milliseconds:item".to_owned(),
                kind: "Generation".to_owned(),
                summary: "等待 65000ms".to_owned(),
                phase: AgentTuiLifecyclePhase::Generation,
                indicator: None,
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            }],
        });

        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.contains("等待 1m 05s"));
        assert!(!text.contains("65000ms"));
    }

    #[test]
    fn streaming_started_hides_provider_model_attempt_noise() {
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:hide-streaming".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:hide-streaming:item".to_owned(),
                kind: "StreamingStarted".to_owned(),
                summary: "cpa/gemini-3.5-flash-low".to_owned(),
                phase: AgentTuiLifecyclePhase::Streaming,
                indicator: None,
                tool_name: Some("cpa/gemini-3.5-flash-low".to_owned()),
                duration_ms: None,
                attempt_index: Some(1),
            }],
        });

        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.is_empty());
    }

    #[test]
    fn streaming_started_surfaces_first_token_latency_after_reply_status() {
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:first-token".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:first-token:item".to_owned(),
                kind: "StreamingStarted".to_owned(),
                summary: "cpa/gpt-5 生成回复中".to_owned(),
                phase: AgentTuiLifecyclePhase::Streaming,
                indicator: None,
                tool_name: Some("gpt-5".to_owned()),
                duration_ms: Some(1_250),
                attempt_index: Some(1),
            }],
        });

        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.contains("首字 1.25s"));
        assert!(!text.contains("cpa/gpt-5"));
        assert!(!text.contains("生成回复中"));
    }

    fn item(kind: &str, summary: &str, phase: AgentTuiLifecyclePhase) -> RenderLifecycleItem {
        RenderLifecycleItem {
            id: format!("test:generation:item:{kind}"),
            kind: kind.to_owned(),
            summary: summary.to_owned(),
            phase,
            indicator: None,
            tool_name: None,
            duration_ms: None,
            attempt_index: None,
        }
    }

    fn test_visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    fn lines_text(lines: Vec<Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn streaming_generation_emits_per_char_shimmer_spans() {
        // Streaming/Generation phase: 文本部分被拆成 shimmer 多 span，
        // 而非静态单 span。
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:shimmer".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:shimmer:item".to_owned(),
                kind: "Generation".to_owned(),
                summary: "等待首字输出".to_owned(),
                phase: AgentTuiLifecyclePhase::Generation,
                indicator: None,
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            }],
        });
        let lines = render_standalone(test_visual(), false, Some(&data), 80);
        assert_eq!(lines.len(), 1);
        // 首 span 是 prefix "  ◌ "，后续 span 数量 = 文本字符数（每个字符一个 span）。
        let span_count = lines[0].spans.len();
        assert!(
            span_count >= 2,
            "streaming shimmer 应至少拆出 prefix + 多个文本 span，实际 span 数：{span_count}"
        );
    }

    #[test]
    fn streaming_generation_omits_reply_status_text() {
        let data = RenderCellData::GenerationStatus(RenderGenerationStatusCell {
            id: "test:generation:shimmer".to_owned(),
            title: "生成".to_owned(),
            items: vec![RenderLifecycleItem {
                id: "test:generation:shimmer:item".to_owned(),
                kind: "Generation".to_owned(),
                summary: "生成回复中".to_owned(),
                phase: AgentTuiLifecyclePhase::Generation,
                indicator: None,
                tool_name: None,
                duration_ms: None,
                attempt_index: None,
            }],
        });
        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 80));

        assert!(text.is_empty());
    }
}
