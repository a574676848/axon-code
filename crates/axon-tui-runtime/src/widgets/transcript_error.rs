//! Error transcript cell 渲染。

use ratatui::{
    style::{Modifier, Style},
    text::Line,
};

use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    surface::{
        cell::RenderCellData,
        cell_event_data::AgentRenderErrorCell,
        visual_density::{BULLET_PREFIX, STATUS_FAILED_PREFIX},
    },
    widgets::cells::error_cell::ErrorCellView,
    widgets::{
        transcript_cells::{normalize_and_truncate_cell_text, render_wrapped_block},
        transcript_style::{CellVisual, role_style},
    },
};

const ERROR_MESSAGE_PREFIX: &str = "  │ ";
const EMPTY_ERROR_MESSAGE: &str = "未收到可展示的错误详情。";
const ERROR_DETAIL_PREFIX: &str = BULLET_PREFIX;
const ERROR_DETAIL_LIMIT: usize = 96;
const MISSING_ERROR_PAYLOAD: &str = "错误详情载荷缺失";

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    if let Some(cell) = error_cell_data(data) {
        return render_typed_error_cell(visual, cell, width);
    }
    let error_style = role_style(TranscriptColorRole::Error).add_modifier(Modifier::BOLD);
    render_error_line(
        STATUS_FAILED_PREFIX,
        MISSING_ERROR_PAYLOAD,
        error_style,
        error_style,
        width,
    )
}

fn error_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderErrorCell> {
    match data {
        Some(RenderCellData::Error(cell)) => Some(cell),
        _ => None,
    }
}

fn render_typed_error_cell(
    visual: CellVisual,
    cell: &AgentRenderErrorCell,
    width: u16,
) -> Vec<Line<'static>> {
    let cell = ErrorCellView::from_cell(cell);
    let error_style = role_style(TranscriptColorRole::Error);
    let message = if cell.message.trim().is_empty() {
        EMPTY_ERROR_MESSAGE
    } else {
        cell.message
    };
    let mut rendered = render_error_line(
        STATUS_FAILED_PREFIX,
        cell.title,
        error_style.add_modifier(Modifier::BOLD),
        error_style.add_modifier(Modifier::BOLD),
        width,
    );
    rendered.extend(render_error_line(
        ERROR_MESSAGE_PREFIX,
        message,
        error_style.add_modifier(Modifier::BOLD),
        error_style,
        width,
    ));
    for detail in error_details_view(&cell) {
        rendered.extend(render_error_line(
            ERROR_DETAIL_PREFIX,
            &detail,
            visual.detail_prefix_style,
            role_style(TranscriptColorRole::Muted),
            width,
        ));
    }
    rendered
}

#[cfg(test)]
fn error_details(cell: &AgentRenderErrorCell) -> Vec<String> {
    let cell = ErrorCellView::from_cell(cell);
    error_details_view(&cell)
}

fn error_details_view(cell: &ErrorCellView<'_>) -> Vec<String> {
    let mut details = Vec::new();
    for value in cell
        .details
        .iter()
        .map(String::as_str)
        .chain(cell.next_step)
    {
        let normalized = normalize_and_truncate_cell_text(value, ERROR_DETAIL_LIMIT);
        if normalized.is_empty()
            || normalized == cell.title
            || normalized == cell.message
            || details.iter().any(|detail| detail == &normalized)
        {
            continue;
        }
        details.push(normalized);
    }
    details
}

fn render_error_line(
    prefix: &str,
    value: &str,
    prefix_style: Style,
    style: Style,
    width: u16,
) -> Vec<Line<'static>> {
    render_wrapped_block(
        prefix,
        &" ".repeat(prefix.len()),
        &normalize_and_truncate_cell_text(value, ERROR_DETAIL_LIMIT),
        prefix_style,
        style,
        width,
    )
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::{
        EMPTY_ERROR_MESSAGE, ERROR_MESSAGE_PREFIX, MISSING_ERROR_PAYLOAD, error_details,
        render_standalone,
    };
    use crate::{
        r#loop::agent_tui::{AgentTuiRunError, test_run_error_cell},
        surface::error_model::{TuiErrorAction, TuiErrorCellKind, TuiErrorSeverity},
        surface::{cell::RenderCellData, cell_event_data::AgentRenderErrorCell},
        widgets::transcript_style::CellVisual,
    };

    #[test]
    fn provider_tool_call_incomplete_uses_structured_error_projection_and_visuals() {
        let run_error = AgentTuiRunError::structured_with_id(
            "error:runtime-event-542",
            "provider tool-call incomplete: call_index=1, missing_field=malformed_arguments",
            Some("provider_transport".to_owned()),
            Some(200),
            Some("provider_tool_call_incomplete".to_owned()),
            Some("malformed_arguments".to_owned()),
        )
        .with_caused_by_runtime_event("runtime-event-542");
        let data = RenderCellData::Error(test_run_error_cell(&run_error));

        let lines = render_standalone(test_visual(), false, Some(&data), 120);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        let error_color = crate::app_shell::style_tokens::transcript_palette()
            .color(crate::app_shell::style_tokens::TranscriptColorRole::Error);

        assert!(matches!(data, RenderCellData::Error(_)));
        assert!(text.contains("  ✗ 模型响应不完整"));
        assert!(text.contains("  │ 模型服务返回了不完整的工具调用，本次未执行该调用。"));
        assert!(text.contains("  · HTTP 200"));
        assert_eq!(lines[0].spans[0].style.fg, Some(error_color));
    }

    #[test]
    fn next_step_is_compacted_into_deduped_details() {
        let cell = AgentRenderErrorCell {
            id: "error:test:details".to_owned(),
            title: "认证失败".to_owned(),
            message: "provider timeout".to_owned(),
            details: vec!["HTTP 401".to_owned(), "检查认证配置".to_owned()],
            next_step: Some("检查认证配置".to_owned()),
            severity: TuiErrorSeverity::UserFacing,
            action: TuiErrorAction::Reconfigure,
            cell_kind: TuiErrorCellKind::Runtime,
        };

        assert_eq!(error_details(&cell), vec!["HTTP 401", "检查认证配置"]);
    }

    #[test]
    fn error_cell_does_not_render_separate_next_step_arrow() {
        let data = RenderCellData::Error(AgentRenderErrorCell {
            id: "error:test:next-step".to_owned(),
            title: "认证失败".to_owned(),
            message: "模型服务拒绝了当前凭据。".to_owned(),
            details: vec!["HTTP 401".to_owned()],
            next_step: Some("检查认证配置".to_owned()),
            severity: TuiErrorSeverity::UserFacing,
            action: TuiErrorAction::Reconfigure,
            cell_kind: TuiErrorCellKind::Runtime,
        });

        let text = render_standalone(test_visual(), false, Some(&data), 120)
            .into_iter()
            .map(|line| {
                line.spans
                    .into_iter()
                    .map(|span| span.content.into_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("· 检查认证配置"));
        assert!(!text.contains("→"));
    }

    #[test]
    fn error_title_and_message_use_error_color_with_hanging_indent() {
        let data = RenderCellData::Error(AgentRenderErrorCell {
            id: "error:test:style".to_owned(),
            title: "MCP 调用失败".to_owned(),
            message: "gitnexus.query 连接被拒绝".to_owned(),
            details: Vec::new(),
            next_step: None,
            severity: TuiErrorSeverity::Recoverable,
            action: TuiErrorAction::InspectDetails,
            cell_kind: TuiErrorCellKind::Runtime,
        });

        let lines = render_standalone(test_visual(), false, Some(&data), 120);
        let error_color = crate::app_shell::style_tokens::transcript_palette()
            .color(crate::app_shell::style_tokens::TranscriptColorRole::Error);

        assert_eq!(lines[0].spans[0].content.as_ref(), "  ✗ ");
        assert_eq!(lines[0].spans[0].style.fg, Some(error_color));
        assert_eq!(lines[0].spans[1].style.fg, Some(error_color));
        assert_eq!(lines[1].spans[0].content.as_ref(), ERROR_MESSAGE_PREFIX);
        assert_eq!(lines[1].spans[1].style.fg, Some(error_color));
    }

    #[test]
    fn empty_error_message_renders_safe_fallback_detail() {
        let data = RenderCellData::Error(AgentRenderErrorCell {
            id: "error:test:empty-message".to_owned(),
            title: "运行失败".to_owned(),
            message: String::new(),
            details: Vec::new(),
            next_step: None,
            severity: TuiErrorSeverity::Recoverable,
            action: TuiErrorAction::None,
            cell_kind: TuiErrorCellKind::Runtime,
        });

        let text = render_standalone(test_visual(), false, Some(&data), 120)
            .into_iter()
            .flat_map(|line| line.spans)
            .map(|span| span.content.into_owned())
            .collect::<String>();

        assert!(text.contains(EMPTY_ERROR_MESSAGE));
    }

    #[test]
    fn missing_typed_error_payload_renders_explicit_failure() {
        let text = render_standalone(test_visual(), false, None, 40)
            .into_iter()
            .flat_map(|line| line.spans)
            .map(|span| span.content.into_owned())
            .collect::<String>();

        assert!(text.contains(MISSING_ERROR_PAYLOAD));
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
