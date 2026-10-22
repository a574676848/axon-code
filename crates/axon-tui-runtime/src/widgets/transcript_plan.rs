//! Plan transcript cell 渲染。

use crate::{
    app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
    surface::{
        cell::RenderCellData,
        cell_plan_data::{AgentRenderPlanCell, AgentRenderPlanStatus, AgentRenderPlanStep},
        visual_density::{STATUS_FAILED_PREFIX, STATUS_PENDING_PREFIX, STATUS_SKIPPED_PREFIX},
    },
    widgets::{
        transcript_cells::{normalize_cell_whitespace, render_wrapped_block},
        transcript_style::{
            CellVisual, role_style, strong_role_style, syntax_role_strong_style, syntax_role_style,
        },
    },
};
use axon_display::normalize_markdown_source;
use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

const PLAN_EXPANDED_LIMIT: usize = usize::MAX;
const PLAN_PREFIX: &str = "  ";

const PLAN_PREFIX_COMPLETED: &str = "  ✔ ";

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    render_plan_cell(visual, data, width, PLAN_EXPANDED_LIMIT)
}

fn render_plan_cell(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    let Some(cell) = plan_cell_data(data) else {
        return Vec::new();
    };
    let mut lines = render_plan_explanation(visual, cell, width);
    let original_count = cell.steps.len();
    lines.extend(
        cell.steps
            .iter()
            .take(limit)
            .flat_map(|step| render_plan_step(visual, step, width))
            .collect::<Vec<_>>(),
    );
    append_omitted(&mut lines, visual, original_count, limit, width);
    lines
}

fn plan_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderPlanCell> {
    match data {
        Some(RenderCellData::Plan(cell)) => Some(cell),
        _ => None,
    }
}

fn render_plan_explanation(
    visual: CellVisual,
    cell: &AgentRenderPlanCell,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(explanation) = cell
        .explanation
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Vec::new();
    };
    render_plan_text_block(
        PLAN_PREFIX,
        PLAN_PREFIX,
        visual.detail_prefix_style,
        visual.detail_prefix_style,
        visual.detail_text_style.dim().italic(),
        explanation,
        width,
    )
}

fn render_plan_step(
    visual: CellVisual,
    step: &AgentRenderPlanStep,
    width: u16,
) -> Vec<Line<'static>> {
    let prefix = plan_prefix_for_status(step.status);
    let prefix_style = plan_style_for_status(step.status);
    let text_style = plan_text_style_for_status(step.status, visual);
    render_plan_text_block(
        prefix,
        PLAN_PREFIX,
        prefix_style,
        visual.detail_prefix_style,
        text_style,
        &step.summary,
        width,
    )
}

fn render_plan_text_block(
    first_prefix: &str,
    continuation_prefix: &str,
    first_prefix_style: Style,
    continuation_prefix_style: Style,
    text_style: Style,
    text: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let normalized = normalize_markdown_source(text);
    let mut lines = Vec::new();
    for (index, logical_line) in normalized.lines().enumerate() {
        if logical_line.trim().is_empty() {
            lines.push(Line::from(""));
            continue;
        }
        lines.extend(render_wrapped_block(
            if index == 0 {
                first_prefix
            } else {
                continuation_prefix
            },
            continuation_prefix,
            &normalize_cell_whitespace(logical_line),
            if index == 0 {
                first_prefix_style
            } else {
                continuation_prefix_style
            },
            text_style,
            width,
        ));
    }
    lines
}

fn append_omitted(
    lines: &mut Vec<Line<'static>>,
    visual: CellVisual,
    original_count: usize,
    limit: usize,
    width: u16,
) {
    let omitted = original_count.saturating_sub(limit);
    if omitted == 0 {
        return;
    }
    lines.extend(render_wrapped_block(
        PLAN_PREFIX,
        &" ".repeat(PLAN_PREFIX.len()),
        &format!("另 {omitted} 项"),
        visual.detail_prefix_style,
        visual.detail_text_style,
        width,
    ));
}

fn plan_prefix_for_status(status: AgentRenderPlanStatus) -> &'static str {
    match status {
        AgentRenderPlanStatus::Completed => PLAN_PREFIX_COMPLETED,
        AgentRenderPlanStatus::Running => STATUS_PENDING_PREFIX,
        AgentRenderPlanStatus::Failed => STATUS_FAILED_PREFIX,
        AgentRenderPlanStatus::Skipped => STATUS_SKIPPED_PREFIX,
        AgentRenderPlanStatus::Pending => STATUS_PENDING_PREFIX,
    }
}

fn plan_style_for_status(status: AgentRenderPlanStatus) -> Style {
    match status {
        AgentRenderPlanStatus::Completed => role_style(TranscriptColorRole::ToolCompleted)
            .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        AgentRenderPlanStatus::Running => syntax_role_strong_style(
            &["keyword.control", "keyword"],
            TranscriptColorRole::Loading,
        ),
        AgentRenderPlanStatus::Failed => strong_role_style(TranscriptColorRole::ToolFailed),
        AgentRenderPlanStatus::Skipped | AgentRenderPlanStatus::Pending => {
            role_style(TranscriptColorRole::Muted).add_modifier(Modifier::DIM)
        }
    }
}

fn plan_text_style_for_status(status: AgentRenderPlanStatus, visual: CellVisual) -> Style {
    let palette = transcript_palette();
    match status {
        AgentRenderPlanStatus::Completed => visual
            .detail_text_style
            .fg(palette.color(TranscriptColorRole::ToolCompleted))
            .add_modifier(Modifier::DIM | Modifier::CROSSED_OUT),
        AgentRenderPlanStatus::Running => visual.detail_text_style.patch(
            syntax_role_style(
                &["keyword.control", "keyword"],
                TranscriptColorRole::Loading,
            )
            .add_modifier(Modifier::BOLD),
        ),
        AgentRenderPlanStatus::Failed => visual
            .detail_text_style
            .fg(palette.color(TranscriptColorRole::ToolFailed))
            .add_modifier(Modifier::BOLD),
        AgentRenderPlanStatus::Skipped | AgentRenderPlanStatus::Pending => visual
            .detail_text_style
            .fg(palette.color(TranscriptColorRole::Muted))
            .add_modifier(Modifier::DIM),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Style, text::Line};

    use super::render_standalone;
    use crate::app_shell::style_tokens::{TranscriptColorRole, transcript_palette};
    use crate::surface::{
        cell::RenderCellData,
        cell_plan_data::{
            AgentRenderPlanCell, AgentRenderPlanSourceKind, AgentRenderPlanStatus,
            AgentRenderPlanStep,
        },
    };
    use crate::widgets::transcript_style::CellVisual;

    #[test]
    fn plan_with_empty_steps_stays_quiet() {
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:empty".to_owned(),
            title: "执行计划".to_owned(),
            explanation: None,
            source: String::new(),
            source_kind: AgentRenderPlanSourceKind::Stream,
            steps: Vec::new(),
        });

        assert!(render_standalone(test_visual(), false, Some(&cell), 80).is_empty());
    }

    #[test]
    fn standalone_plan_keeps_full_plan_without_outer_update_header() {
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:completed".to_owned(),
            title: "计划".to_owned(),
            explanation: Some("按 Codex 计划块显示说明。".to_owned()),
            source: "# 计划".to_owned(),
            source_kind: AgentRenderPlanSourceKind::Markdown,
            steps: vec![
                AgentRenderPlanStep {
                    id: None,
                    status: AgentRenderPlanStatus::Running,
                    summary: "当前任务".to_owned(),
                },
                AgentRenderPlanStep {
                    id: None,
                    status: AgentRenderPlanStatus::Pending,
                    summary: "下一步".to_owned(),
                },
            ],
        });

        let text = plan_lines_text(render_standalone(test_visual(), false, Some(&cell), 80));

        assert!(text.contains("按 Codex 计划块"));
        assert!(text.contains("当前任务"));
        assert!(text.contains("下一步"));
        assert!(!text.starts_with("• 更新"));
    }

    #[test]
    fn plan_steps_render_terminal_and_skipped_lifecycle_states() {
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:source".to_owned(),
            title: "计划".to_owned(),
            explanation: None,
            source: "# 计划".to_owned(),
            source_kind: AgentRenderPlanSourceKind::Markdown,
            steps: vec![
                AgentRenderPlanStep {
                    id: None,
                    status: AgentRenderPlanStatus::Failed,
                    summary: "审批阻断".to_owned(),
                },
                AgentRenderPlanStep {
                    id: None,
                    status: AgentRenderPlanStatus::Skipped,
                    summary: "跳过低价值验证".to_owned(),
                },
            ],
        });

        let text = plan_lines_text(render_standalone(test_visual(), false, Some(&cell), 80));

        assert!(text.contains("✗ 审批阻断"));
        assert!(text.contains("- 跳过低价值验证"));
    }

    #[test]
    fn plan_steps_style_like_codex_checkbox_todos() {
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:compact".to_owned(),
            title: "计划".to_owned(),
            explanation: None,
            source: "# 计划\n\n- [x] 已完成".to_owned(),
            source_kind: AgentRenderPlanSourceKind::Markdown,
            steps: vec![
                AgentRenderPlanStep {
                    id: None,
                    status: AgentRenderPlanStatus::Completed,
                    summary: "完成分析".to_owned(),
                },
                AgentRenderPlanStep {
                    id: None,
                    status: AgentRenderPlanStatus::Pending,
                    summary: "等待验证".to_owned(),
                },
            ],
        });

        let lines = render_standalone(test_visual(), false, Some(&cell), 80);

        assert_eq!(lines[0].spans[0].content.as_ref(), "  ✔ ");
        assert!(
            lines[0].spans[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::CROSSED_OUT)
        );
        assert!(
            lines[0].spans[1]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::CROSSED_OUT)
        );
        assert_eq!(lines[1].spans[0].content.as_ref(), "  □ ");
        assert_eq!(
            lines[1].spans[0].style.fg,
            Some(transcript_palette().color(TranscriptColorRole::Muted))
        );
        assert_eq!(
            lines[1].spans[1].style.fg,
            Some(transcript_palette().color(TranscriptColorRole::Muted))
        );
    }

    #[test]
    fn plan_render_step_contains_only_status_and_summary() {
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:selected".to_owned(),
            title: "计划".to_owned(),
            explanation: None,
            source: "# 计划".to_owned(),
            source_kind: AgentRenderPlanSourceKind::Markdown,
            steps: vec![AgentRenderPlanStep {
                id: None,
                status: AgentRenderPlanStatus::Running,
                summary: "#3".to_owned(),
            }],
        });

        let text = plan_lines_text(render_standalone(test_visual(), false, Some(&cell), 80));

        assert!(text.contains("□ #3"));
        assert!(!text.contains("诊断"));
        assert!(!text.contains("回滚"));
    }

    #[test]
    fn plan_step_preserves_dense_labeled_metadata_as_logical_lines() {
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:dense-metadata".to_owned(),
            title: "计划".to_owned(),
            explanation: None,
            source: String::new(),
            source_kind: AgentRenderPlanSourceKind::Stream,
            steps: vec![AgentRenderPlanStep {
                id: None,
                status: AgentRenderPlanStatus::Running,
                summary:
                    "[TASK-001]: 克隆 grok-build 仓库 - 状态:进行中 - 目标: E:\\zbg\\grok-build"
                        .to_owned(),
            }],
        });

        let lines = render_standalone(test_visual(), false, Some(&cell), 80);
        let text = plan_lines_text(lines);
        let logical_lines = text.lines().collect::<Vec<_>>();

        assert!(logical_lines.iter().any(|line| line.contains("[TASK-001]")));
        assert!(
            logical_lines
                .iter()
                .any(|line| line.contains("- 状态:进行中"))
        );
        assert!(logical_lines.iter().any(|line| line.contains("- 目标:")));
        assert!(
            logical_lines
                .iter()
                .all(|line| !(line.contains("状态:进行中") && line.contains("目标:")))
        );
    }

    #[test]
    fn plan_step_wraps_full_long_cjk_summary_at_narrow_width() {
        let summary = "确保中文计划的每一项内容在窄窗口中完整换行展示而不丢失末尾信息".repeat(4);
        let cell = RenderCellData::Plan(AgentRenderPlanCell {
            id: "test:plan:long-cjk".to_owned(),
            title: "计划".to_owned(),
            explanation: None,
            source: String::new(),
            source_kind: AgentRenderPlanSourceKind::Stream,
            steps: vec![AgentRenderPlanStep {
                id: None,
                status: AgentRenderPlanStatus::Pending,
                summary: summary.clone(),
            }],
        });

        let lines = render_standalone(test_visual(), false, Some(&cell), 24);
        let rendered_summary = lines
            .iter()
            .flat_map(|line| line.spans.iter().skip(1))
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(rendered_summary, summary);
        assert!(lines.iter().all(|line| {
            let rendered_line = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            crate::widgets::transcript_cells::display_width(&rendered_line) <= 24
        }));
    }

    fn test_visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    fn plan_lines_text(lines: Vec<Line<'static>>) -> String {
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
}
