//! Review transcript cell 渲染。

use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    surface::{
        cell::RenderCellData,
        cell_review_data::{
            AgentRenderReviewCell, AgentRenderReviewDetail, AgentRenderReviewDetailKind,
            AgentRenderReviewStatus,
        },
        visual_density::{
            DETAIL_CONTINUATION, NEUTRAL_PREFIX, STATUS_APPROVED_PREFIX, STATUS_FAILED_PREFIX,
            STATUS_PENDING_PREFIX, STATUS_RUNNING_PREFIX,
        },
    },
    widgets::{
        transcript_cells::{normalize_and_truncate_cell_text, render_wrapped_block},
        transcript_style::{CellVisual, role_style, strong_role_style},
    },
};

const REVIEW_EXPANDED_LIMIT: usize = usize::MAX;
const REVIEW_PREFIX: &str = "  ";
const REVIEW_DETAIL_LIMIT: usize = 72;

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(cell) = review_cell_data(data) else {
        return Vec::new();
    };
    render_review_cell(visual, cell, width, REVIEW_EXPANDED_LIMIT)
}

fn review_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderReviewCell> {
    match data {
        Some(RenderCellData::Review(cell)) => Some(cell),
        _ => None,
    }
}

fn render_review_cell(
    visual: CellVisual,
    cell: &AgentRenderReviewCell,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    let mut lines = render_wrapped_block(
        review_prefix_for_status(cell.status),
        REVIEW_PREFIX,
        &normalize_and_truncate_cell_text(&cell.subject, REVIEW_DETAIL_LIMIT),
        review_prefix_style(cell.status, visual),
        review_text_style(cell.status, visual),
        width,
    );
    let details = review_details(cell)
        .into_iter()
        .filter(|detail| !detail_is_duplicate_status(&cell.subject, &detail.text))
        .collect::<Vec<_>>();
    append_review_detail_lines(&mut lines, visual, details, cell.status, width, limit);
    lines
}

fn review_details(cell: &AgentRenderReviewCell) -> Vec<AgentRenderReviewDetail> {
    cell.details
        .iter()
        .filter(|detail| !detail_is_duplicate_status(&cell.subject, &detail.text))
        .cloned()
        .collect()
}

fn append_review_detail_lines(
    lines: &mut Vec<Line<'static>>,
    visual: CellVisual,
    details: Vec<AgentRenderReviewDetail>,
    status: Option<AgentRenderReviewStatus>,
    width: u16,
    limit: usize,
) {
    let original_count = details.len();
    lines.extend(details.into_iter().take(limit).flat_map(|detail| {
        render_wrapped_block(
            review_detail_kind_prefix(detail.kind),
            REVIEW_PREFIX,
            &normalize_and_truncate_cell_text(&detail.text, REVIEW_DETAIL_LIMIT),
            review_prefix_style(status, visual),
            review_text_style(status, visual),
            width,
        )
    }));
    append_omitted(lines, visual, original_count, limit, width);
}

fn review_detail_kind_prefix(kind: AgentRenderReviewDetailKind) -> &'static str {
    match kind {
        AgentRenderReviewDetailKind::Command => "  $ ",
        AgentRenderReviewDetailKind::Diagnostic => "  ! ",
        AgentRenderReviewDetailKind::Delta => "  Δ ",
        AgentRenderReviewDetailKind::Message => DETAIL_CONTINUATION,
    }
}

fn review_prefix_for_status(status: Option<AgentRenderReviewStatus>) -> &'static str {
    match status {
        Some(AgentRenderReviewStatus::Approved | AgentRenderReviewStatus::Completed) => {
            STATUS_APPROVED_PREFIX
        }
        Some(
            AgentRenderReviewStatus::Denied
            | AgentRenderReviewStatus::Failed
            | AgentRenderReviewStatus::Blocked
            | AgentRenderReviewStatus::Stopped,
        ) => STATUS_FAILED_PREFIX,
        Some(AgentRenderReviewStatus::Running) => STATUS_RUNNING_PREFIX,
        Some(AgentRenderReviewStatus::Pending) => STATUS_PENDING_PREFIX,
        Some(AgentRenderReviewStatus::Unknown) | None => NEUTRAL_PREFIX,
    }
}

fn review_prefix_style(status: Option<AgentRenderReviewStatus>, visual: CellVisual) -> Style {
    match status {
        Some(AgentRenderReviewStatus::Approved | AgentRenderReviewStatus::Completed) => {
            role_style(TranscriptColorRole::ToolCompleted).add_modifier(Modifier::DIM)
        }
        Some(
            AgentRenderReviewStatus::Denied
            | AgentRenderReviewStatus::Failed
            | AgentRenderReviewStatus::Blocked
            | AgentRenderReviewStatus::Stopped,
        ) => strong_role_style(TranscriptColorRole::ToolFailed),
        Some(AgentRenderReviewStatus::Running) => {
            Style::default().add_modifier(Modifier::DIM | Modifier::BOLD)
        }
        Some(AgentRenderReviewStatus::Pending) => Style::default().add_modifier(Modifier::DIM),
        Some(AgentRenderReviewStatus::Unknown) | None => {
            visual.detail_prefix_style.add_modifier(Modifier::DIM)
        }
    }
}

fn review_text_style(status: Option<AgentRenderReviewStatus>, _visual: CellVisual) -> Style {
    let style = Style::default().add_modifier(Modifier::DIM);
    match status {
        Some(AgentRenderReviewStatus::Approved | AgentRenderReviewStatus::Completed) => {
            style.add_modifier(Modifier::CROSSED_OUT)
        }
        _ => style,
    }
}

fn detail_is_duplicate_status(subject: &str, detail: &str) -> bool {
    let normalized = detail.trim();
    !normalized.is_empty() && subject.trim() == normalized
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
        REVIEW_PREFIX,
        &" ".repeat(REVIEW_PREFIX.len()),
        &format!("另 {omitted} 项"),
        visual.detail_prefix_style,
        visual.detail_text_style,
        width,
    ));
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Style, text::Line};

    use super::{render_review_cell, render_standalone};
    use crate::app_shell::style_tokens::{TranscriptColorRole, transcript_palette};
    use crate::surface::{
        cell::RenderCellData,
        cell_review_data::{
            AgentRenderReviewCell, AgentRenderReviewDetail, AgentRenderReviewDetailKind,
            AgentRenderReviewStatus,
        },
    };
    use crate::widgets::transcript_style::CellVisual;

    #[test]
    fn review_details_compact_long_validation_output() {
        let cell = RenderCellData::Review(AgentRenderReviewCell {
            id: "test:review:wrap".to_owned(),
            subject: "验证失败".to_owned(),
            status: Some(AgentRenderReviewStatus::Failed),
            details: vec![AgentRenderReviewDetail::text(
                "cargo test failed after running a very long diagnostics report with many repeated low value lines that should stay out of the default transcript",
            )],
        });

        let text = review_lines_text(render_standalone(test_visual(), false, Some(&cell), 120));

        assert!(text.contains('…'));
        assert!(!text.contains("default transcript"));
    }

    #[test]
    fn stopped_review_without_details_renders_status_without_fake_completion() {
        let cell = RenderCellData::Review(AgentRenderReviewCell {
            id: "test:review:quiet".to_owned(),
            subject: "停止 · 停滞".to_owned(),
            status: Some(AgentRenderReviewStatus::Stopped),
            details: Vec::new(),
        });

        let text = review_lines_text(render_standalone(test_visual(), false, Some(&cell), 80));

        assert_eq!(text, "  ✗ 停止 · 停滞");
        assert!(!text.contains("完成"));
        assert!(!text.contains("通过"));
    }

    #[test]
    fn review_without_details_does_not_repeat_status_as_detail() {
        let pending = RenderCellData::Review(AgentRenderReviewCell {
            id: "test:review:pending".to_owned(),
            subject: "审批".to_owned(),
            status: Some(AgentRenderReviewStatus::Pending),
            details: Vec::new(),
        });
        let running = RenderCellData::Review(AgentRenderReviewCell {
            id: "test:review:status-running".to_owned(),
            subject: "验证".to_owned(),
            status: Some(AgentRenderReviewStatus::Running),
            details: Vec::new(),
        });

        let pending_text =
            review_lines_text(render_standalone(test_visual(), false, Some(&pending), 80));
        let running_text =
            review_lines_text(render_standalone(test_visual(), false, Some(&running), 80));

        assert_eq!(pending_text, "  □ 审批");
        assert_eq!(running_text, "  ◌ 验证");
        assert!(!pending_text.contains("待审批"));
        assert!(!running_text.contains("进行中"));
    }

    #[test]
    fn standalone_review_uses_subject_as_status_line_without_outer_header() {
        let cell = RenderCellData::Review(AgentRenderReviewCell {
            id: "test:review:approved".to_owned(),
            subject: "验证失败".to_owned(),
            status: Some(AgentRenderReviewStatus::Failed),
            details: vec![AgentRenderReviewDetail::text("cargo check")],
        });

        let text = review_lines_text(render_standalone(test_visual(), false, Some(&cell), 80));

        assert!(text.starts_with("  ✗ 验证失败"));
        assert!(text.contains("cargo check"));
        assert!(!text.starts_with("• 验证失败"));
    }

    #[test]
    fn review_drops_lifecycle_and_prefix_noise() {
        let cell = AgentRenderReviewCell {
            id: "test:review:quiet".to_owned(),
            subject: "验证".to_owned(),
            status: Some(AgentRenderReviewStatus::Completed),
            details: vec![
                AgentRenderReviewDetail::text("lsp_diagnostics"),
                AgentRenderReviewDetail {
                    kind: AgentRenderReviewDetailKind::Diagnostic,
                    text: "diagnostic".to_owned(),
                },
            ],
        };

        let text = review_lines_text(render_review_cell(test_visual(), &cell, 80, 8));

        assert!(!text.contains("通过"));
        assert!(text.contains("验证"));
        assert!(text.contains("lsp_diagnostics"));
        assert!(text.contains("diagnostic"));
        assert!(!text.contains("completed"));
        assert!(!text.contains("工具 lsp_diagnostics"));
        assert!(!text.contains("状态 completed"));
        assert!(!text.contains("错误 diagnostic"));
    }

    #[test]
    fn review_does_not_repeat_subject_status() {
        let cell = AgentRenderReviewCell {
            id: "test:review:compact".to_owned(),
            subject: "验证失败".to_owned(),
            status: Some(AgentRenderReviewStatus::Failed),
            details: vec![
                AgentRenderReviewDetail::text("验证失败"),
                AgentRenderReviewDetail {
                    kind: AgentRenderReviewDetailKind::Diagnostic,
                    text: "诊断 1".to_owned(),
                },
            ],
        };

        let text = review_lines_text(render_review_cell(test_visual(), &cell, 80, 8));

        assert!(!text.contains("• 失败"));
        assert_eq!(text.matches("验证失败").count(), 1);
        assert!(text.contains("诊断 1"));
    }

    #[test]
    fn review_styles_status_like_codex_waiting_cells() {
        let failed = AgentRenderReviewCell {
            id: "test:review:failed".to_owned(),
            subject: "验证".to_owned(),
            status: Some(AgentRenderReviewStatus::Failed),
            details: Vec::new(),
        };
        let running = AgentRenderReviewCell {
            id: "test:review:running-badge".to_owned(),
            subject: "验证".to_owned(),
            status: Some(AgentRenderReviewStatus::Running),
            details: Vec::new(),
        };

        let failed_lines = render_review_cell(test_visual(), &failed, 80, 8);
        let running_lines = render_review_cell(test_visual(), &running, 80, 8);

        assert_eq!(failed_lines[0].spans[0].content.as_ref(), "  ✗ ");
        assert_eq!(
            failed_lines[0].spans[0].style.fg,
            Some(transcript_palette().color(TranscriptColorRole::ToolFailed))
        );
        assert_eq!(running_lines[0].spans[0].content.as_ref(), "  ◌ ");
        assert_eq!(running_lines[0].spans[0].style.fg, None);
        assert!(
            running_lines[0].spans[0]
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::DIM)
        );
    }

    #[test]
    fn review_uses_semantic_detail_prefixes() {
        let cell = AgentRenderReviewCell {
            id: "test:review:selected".to_owned(),
            subject: "验证失败".to_owned(),
            status: Some(AgentRenderReviewStatus::Failed),
            details: vec![AgentRenderReviewDetail {
                kind: AgentRenderReviewDetailKind::Command,
                text: "cargo check".to_owned(),
            }],
        };

        let lines = render_review_cell(test_visual(), &cell, 80, 8);

        assert_eq!(lines[1].spans[0].content.as_ref(), "  $ ");
        assert!(line_text(&lines[1]).contains("cargo check"));
    }

    fn test_visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    fn review_lines_text(lines: Vec<Line<'static>>) -> String {
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

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
