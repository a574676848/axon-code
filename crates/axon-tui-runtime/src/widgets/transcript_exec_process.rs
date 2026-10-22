//! Exec process 状态面渲染。

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::app_shell::syntax_theme::syntax_style_for_scopes;
use crate::{
    app_shell::style_tokens::TranscriptColorRole,
    r#loop::agent_tui::{
        display_units::normalize_display_units_in_text,
        duration_format::format_duration_millis_compact,
    },
    surface::{
        cell_event_data::{AgentRenderExecCell, AgentRenderExecStatus},
        visual_density::{DETAIL_CONTINUATION, DETAIL_PREFIX},
    },
    widgets::{
        terminal_hyperlinks::{prefix_hyperlink_lines, visible_lines},
        transcript_cells::{
            CELL_DETAIL_LIMIT, display_width, normalize_and_truncate_cell_text,
            render_wrapped_block,
        },
        transcript_markdown::render_markdown_hyperlink_lines,
        transcript_style::{CellVisual, role_style, strong_role_style},
    },
};

const EXEC_DETAIL_LIMIT: usize = 200;
const EXEC_ARGS_MARKER: &str = "$";
const EXEC_OUTPUT_MARKER: &str = "│";
const EXEC_OUTPUT_DETAIL_PREFIX: &str = "  │ ";
const EXEC_OUTPUT_DETAIL_CONTINUATION: &str = "    ";
const EXEC_PENDING_MARKER: &str = "◌";
const EXEC_FAILED_MARKER: &str = "✗";
const EXEC_SUCCESS_MARKER: &str = "✓";
const EXEC_DEFAULT_LABEL: &str = "执行";
const EXEC_FAILED_TEXT: &str = "Failed";
const EXEC_RUNNING_TEXT: &str = "Running";
const EXEC_SUMMARY_OUTPUT_LABEL: &str = "Output";
const EXEC_SUMMARY_OUTPUT_PREVIEW_LIMIT: usize = 120;
const EXEC_ARGUMENT_SUMMARY_LIMIT: usize = 120;
const TERMINAL_INTERRUPT_DISPLAY: &str = "Ctrl-C";
const EXEC_ARGUMENT_PRIMARY_KEYS: [&str; 8] = [
    "command", "path", "query", "url", "pattern", "skill", "target", "task",
];

pub(super) fn exec_process_title(cell: &AgentRenderExecCell) -> String {
    let label = cell.display_label.trim();
    let label = if label.is_empty() {
        EXEC_DEFAULT_LABEL
    } else {
        label
    };
    match cell
        .display_target
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(target) => format!("{label} · {target}"),
        None => label.to_owned(),
    }
}

pub(super) fn render_exec_process_body(
    visual: CellVisual,
    cell: &AgentRenderExecCell,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(status_text) = exec_status_detail_text(cell) {
        lines.extend(render_exec_detail(
            visual,
            exec_status_marker(cell.status),
            &status_text,
            width,
            exec_status_style(cell.status),
            true,
        ));
    }
    if let Some(arguments) = cell.arguments_preview.as_deref() {
        lines.extend(render_exec_detail(
            visual,
            EXEC_ARGS_MARKER,
            arguments,
            width,
            role_style(TranscriptColorRole::ToolInput),
            false,
        ));
    }
    if let Some(output) = cell.output_preview.as_deref() {
        lines.extend(render_exec_output_detail(
            visual,
            output,
            width,
            output_detail_style(),
        ));
    }
    if let Some(detail) = raw_tool_detail_text(cell) {
        lines.extend(render_exec_detail(
            visual,
            EXEC_OUTPUT_MARKER,
            &detail,
            width,
            output_detail_style(),
            false,
        ));
    }
    if let Some(result) = exec_result_text(cell) {
        lines.extend(render_exec_detail(
            visual,
            exec_result_marker(cell.status),
            &result,
            width,
            exec_result_style(cell.status),
            matches!(cell.status, AgentRenderExecStatus::Failed),
        ));
    }
    lines
}

fn collapsed_arguments_text(cell: &AgentRenderExecCell) -> Option<String> {
    // 折叠态主事实已经落在标题行上（模型自述目的、工具专用规则或 MCP 名）时不再重复参数：
    // 命令原文与原始 JSON 都在展开态完整可见。
    if axon_display::tool_row_title_carries_primary(
        &cell.raw_tool_name,
        cell.arguments_preview.as_deref().unwrap_or_default(),
    ) {
        return None;
    }
    let arguments = cell
        .arguments_preview
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "{}");
    let display = if let Some(arguments) = arguments {
        if is_write_stdin_tool(cell) {
            terminal_interaction_argument(arguments)?
        } else if is_shell_tool(cell) {
            crate::r#loop::agent_tui::tool_output_projection::shared_executed_command_preview(
                arguments,
            )
        } else {
            preferred_argument_value(arguments).unwrap_or_else(|| arguments.to_owned())
        }
    } else if is_path_tool(cell) {
        cell.output_preview
            .as_deref()
            .and_then(preferred_path_value)?
    } else {
        return None;
    };
    let normalized = display.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut preview = normalized
        .chars()
        .take(EXEC_ARGUMENT_SUMMARY_LIMIT)
        .collect::<String>();
    if normalized.chars().count() > EXEC_ARGUMENT_SUMMARY_LIMIT {
        preview.push_str("...");
    }
    (!preview.is_empty()).then_some(preview)
}

fn is_write_stdin_tool(cell: &AgentRenderExecCell) -> bool {
    cell.raw_tool_name
        .rsplit(['.', ':', '/'])
        .next()
        .is_some_and(|name| name == "write_stdin")
}

fn terminal_interaction_argument(arguments: &str) -> Option<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return Some(arguments.to_owned()).filter(|value| !value.trim().is_empty());
    };
    if value
        .get("ctrl_c")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Some(TERMINAL_INTERRUPT_DISPLAY.to_owned());
    }
    value
        .get("chars")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned)
        .filter(|value| !value.is_empty())
}

fn is_shell_tool(cell: &AgentRenderExecCell) -> bool {
    let name = cell.raw_tool_name.to_ascii_lowercase();
    name.contains("shell")
        || name
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix == "run")
        || name == "run"
}

fn is_path_tool(cell: &AgentRenderExecCell) -> bool {
    matches!(
        cell.raw_tool_name
            .rsplit_once('.')
            .map(|(_, suffix)| suffix)
            .unwrap_or(cell.raw_tool_name.as_str()),
        "read_file" | "glob" | "edit"
    )
}

fn preferred_path_value(value: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(value).ok()?;
    parsed
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

fn preferred_argument_value(arguments: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(arguments).ok()?;
    for key in EXEC_ARGUMENT_PRIMARY_KEYS {
        let Some(candidate) = value.get(key) else {
            continue;
        };
        if let Some(text) = candidate.as_str().filter(|text| !text.trim().is_empty()) {
            return Some(text.to_owned());
        }
        if !candidate.is_null() {
            return Some(candidate.to_string());
        }
    }
    None
}

pub(super) fn render_exec_process_summary(
    visual: CellVisual,
    cell: &AgentRenderExecCell,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(arguments) = collapsed_arguments_text(cell) {
        lines.extend(render_exec_detail(
            visual,
            EXEC_ARGS_MARKER,
            &arguments,
            width,
            role_style(TranscriptColorRole::ToolInput),
            false,
        ));
    }
    if let Some(status_text) = exec_status_detail_text(cell) {
        lines.extend(render_exec_detail(
            visual,
            exec_status_marker(cell.status),
            &status_text,
            width,
            exec_status_style(cell.status),
            true,
        ));
    }
    if let Some(result) = exec_result_text(cell) {
        lines.extend(render_exec_detail(
            visual,
            exec_result_marker(cell.status),
            &result,
            width,
            exec_result_style(cell.status),
            matches!(cell.status, AgentRenderExecStatus::Failed),
        ));
    }
    if let Some(output) = cell.output_preview.as_deref() {
        let text = if is_shell_tool(cell) {
            shell_output_summary_text(output)
        } else {
            Some(output_summary_text(output))
        };
        if let Some(text) = text {
            lines.extend(render_exec_detail(
                visual,
                EXEC_OUTPUT_MARKER,
                &text,
                width,
                output_detail_style(),
                false,
            ));
        }
    }
    lines
}

fn output_summary_text(output: &str) -> String {
    if let Some(path) = preferred_path_value(output) {
        return path;
    }
    let lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let Some(first_line) = lines.first() else {
        return format!("{EXEC_SUMMARY_OUTPUT_LABEL} 无有效内容");
    };
    let preview = first_line
        .chars()
        .take(EXEC_SUMMARY_OUTPUT_PREVIEW_LIMIT)
        .collect::<String>();
    let truncated = first_line.chars().count() > EXEC_SUMMARY_OUTPUT_PREVIEW_LIMIT;
    let suffix = if lines.len() > 1 || truncated {
        format!(" · 共 {} 行", lines.len())
    } else {
        String::new()
    };
    format!("{EXEC_SUMMARY_OUTPUT_LABEL} {preview}{suffix}")
}

fn shell_output_summary_text(output: &str) -> Option<String> {
    let lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    let first_line = lines.first()?;
    let preview = first_line
        .chars()
        .take(EXEC_SUMMARY_OUTPUT_PREVIEW_LIMIT)
        .collect::<String>();
    let truncated = first_line.chars().count() > EXEC_SUMMARY_OUTPUT_PREVIEW_LIMIT;
    let suffix = if lines.len() > 1 || truncated {
        format!(" · 共 {} 行", lines.len())
    } else {
        String::new()
    };
    Some(format!("{preview}{suffix}"))
}

fn exec_result_text(cell: &AgentRenderExecCell) -> Option<String> {
    if matches!(cell.status, AgentRenderExecStatus::Running) {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(summary) = cell
        .summary
        .as_deref()
        .filter(|summary| !summary.trim().is_empty())
    {
        parts.push(normalize_display_units_in_text(summary.trim()));
    }
    if let Some(duration) = cell.duration_ms {
        parts.push(format_exec_duration(duration));
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn raw_tool_detail_text(cell: &AgentRenderExecCell) -> Option<String> {
    cell.display_detail
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn running_status_text(cell: &AgentRenderExecCell) -> String {
    let mut parts = vec![EXEC_RUNNING_TEXT.to_owned()];
    if let Some(duration) = cell.duration_ms {
        parts.push(format_exec_duration(duration));
    }
    parts.join(" · ")
}

fn exec_result_marker(status: AgentRenderExecStatus) -> &'static str {
    match status {
        AgentRenderExecStatus::Failed => EXEC_FAILED_MARKER,
        _ => EXEC_SUCCESS_MARKER,
    }
}

fn exec_result_style(status: AgentRenderExecStatus) -> Style {
    match status {
        AgentRenderExecStatus::Failed => strong_role_style(TranscriptColorRole::ToolFailed),
        _ => strong_role_style(TranscriptColorRole::ToolCompleted),
    }
}

fn exec_status_detail_text(cell: &AgentRenderExecCell) -> Option<String> {
    match cell.status {
        AgentRenderExecStatus::Running => Some(running_status_text(cell)),
        AgentRenderExecStatus::Failed => Some(EXEC_FAILED_TEXT.to_owned()),
        AgentRenderExecStatus::Completed | AgentRenderExecStatus::Output => None,
    }
}

fn exec_status_marker(status: AgentRenderExecStatus) -> &'static str {
    match status {
        AgentRenderExecStatus::Failed => EXEC_FAILED_MARKER,
        AgentRenderExecStatus::Running => EXEC_PENDING_MARKER,
        AgentRenderExecStatus::Completed | AgentRenderExecStatus::Output => EXEC_SUCCESS_MARKER,
    }
}

fn exec_status_style(status: AgentRenderExecStatus) -> Style {
    match status {
        AgentRenderExecStatus::Running => strong_role_style(TranscriptColorRole::ToolRunning),
        AgentRenderExecStatus::Failed => strong_role_style(TranscriptColorRole::ToolFailed),
        AgentRenderExecStatus::Completed | AgentRenderExecStatus::Output => {
            role_style(TranscriptColorRole::Muted)
        }
    }
}

fn render_exec_detail(
    visual: CellVisual,
    marker: &str,
    value: &str,
    width: u16,
    style: Style,
    strong_prefix: bool,
) -> Vec<Line<'static>> {
    let text = normalize_and_truncate_cell_text(value, EXEC_DETAIL_LIMIT.min(CELL_DETAIL_LIMIT));
    let prefix_style = if marker == EXEC_ARGS_MARKER {
        syntax_style_for_scopes(
            &["keyword.operator", "storage.modifier"],
            Style::default().fg(Color::Magenta),
        )
    } else if strong_prefix {
        visual.detail_prefix_style.add_modifier(Modifier::BOLD)
    } else {
        visual.detail_prefix_style
    };
    render_wrapped_block(
        DETAIL_PREFIX,
        DETAIL_CONTINUATION,
        &format!("{marker} {text}"),
        prefix_style,
        style,
        width,
    )
}

fn output_detail_style() -> Style {
    role_style(TranscriptColorRole::Muted).add_modifier(Modifier::DIM)
}

fn render_exec_output_detail(
    visual: CellVisual,
    value: &str,
    width: u16,
    style: Style,
) -> Vec<Line<'static>> {
    let prefix_width = u16::try_from(display_width(EXEC_OUTPUT_DETAIL_PREFIX)).unwrap_or(u16::MAX);
    let body_width = width.saturating_sub(prefix_width).max(1);
    visible_lines(&prefix_hyperlink_lines(
        render_markdown_hyperlink_lines(value, style, body_width, None),
        Span::styled(EXEC_OUTPUT_DETAIL_PREFIX, visual.detail_prefix_style),
        Span::raw(EXEC_OUTPUT_DETAIL_CONTINUATION),
    ))
}

fn format_exec_duration(duration_ms: u64) -> String {
    format_duration_millis_compact(duration_ms)
}

#[cfg(test)]
mod tests {
    use ratatui::style::Style;

    use super::{
        exec_process_title, output_summary_text, render_exec_output_detail,
        render_exec_process_body, render_exec_process_summary,
    };
    use crate::{
        app_shell::style_tokens::{TranscriptColorRole, transcript_palette},
        surface::cell_event_data::AgentRenderExecCell,
        widgets::transcript_style::CellVisual,
    };

    use super::shell_output_summary_text;

    fn lines_text(lines: Vec<ratatui::text::Line<'static>>) -> String {
        lines
            .into_iter()
            .flat_map(|line| line.spans.into_iter())
            .map(|span| span.content.into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn shell_output_summary_renders_raw_output_without_output_label_or_prefixes() {
        assert_eq!(
            shell_output_summary_text(
                "\n   Compiling axon-cli v0.1.0\n    Finished test profile\ntest result: ok"
            ),
            Some("Compiling axon-cli v0.1.0 · 共 3 行".to_owned())
        );
        assert_eq!(shell_output_summary_text("\n  \n"), None);
    }

    #[test]
    fn render_exec_process_summary_for_shell_tool_omits_output_label() {
        let mut cell = AgentRenderExecCell::running_with_id(
            "exec:test:shell-raw",
            "builtin.shell_command",
            "tool-1",
            Some(r#"{"command":"cargo test"}"#.to_owned()),
        );
        cell.output_preview = Some("test result: ok. 1 passed · 共 4 行".to_owned());
        let lines = render_exec_process_summary(test_visual(), &cell, 80);
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(!text.contains("Output"));
        assert!(!text.contains("出"));
        assert!(!text.contains("错"));
        assert!(text.contains("test result: ok"));
    }

    #[test]
    fn collapsed_shell_summary_hides_command_when_intent_carries_the_intent() {
        let mut cell = AgentRenderExecCell::running_with_id(
            "exec:test:intent",
            "builtin.shell_command",
            "tool-1",
            Some(r#"{"command":"cargo fmt --all --check","intent":"校验代码格式"}"#.to_owned()),
        );
        cell.display_target = Some("校验代码格式".to_owned());
        let collapsed = lines_text(render_exec_process_summary(test_visual(), &cell, 80));
        let expanded = lines_text(render_exec_process_body(test_visual(), &cell, 80));

        assert!(!collapsed.contains("cargo fmt --all --check"));
        assert!(
            expanded.contains("cargo fmt --all --check"),
            "展开态必须能看到真实命令"
        );
    }

    #[test]
    fn collapsed_row_hides_raw_arguments_when_a_shared_rule_carries_the_primary() {
        let mut glob = AgentRenderExecCell::running_with_id(
            "exec:test:glob-intent",
            "builtin.glob",
            "tool-1",
            Some(r#"{"gitignore":false,"paths":["docs/**/*.md"],"intent":"列出文档"}"#.to_owned()),
        );
        glob.display_target = Some("列出文档".to_owned());
        let collapsed = lines_text(render_exec_process_summary(test_visual(), &glob, 80));
        let expanded = lines_text(render_exec_process_body(test_visual(), &glob, 80));

        assert_eq!(exec_process_title(&glob), "匹配 · 列出文档");
        assert!(!collapsed.contains("gitignore"), "折叠态不再摆参数 JSON");
        assert!(expanded.contains("gitignore"), "展开态必须保留原始参数");

        let silent = AgentRenderExecCell::running_with_id(
            "exec:test:silent",
            "builtin.get_context_remaining",
            "tool-1",
            Some(r#"{"__axon_tool_call_parse_error":"empty_arguments"}"#.to_owned()),
        );
        let collapsed = lines_text(render_exec_process_summary(test_visual(), &silent, 80));
        assert_eq!(exec_process_title(&silent), "查看剩余上下文");
        assert!(!collapsed.contains("__axon_tool_call_parse_error"));
        assert!(!collapsed.contains("empty_arguments"));
    }

    #[test]
    fn memory_search_row_shows_the_query_once() {
        let mut cell = AgentRenderExecCell::running_with_id(
            "exec:test:memory-search",
            "builtin.memory_search",
            "tool-1",
            Some(r#"{"limit":8,"query":"仓库文档重整"}"#.to_owned()),
        );
        cell.display_target = Some("仓库文档重整".to_owned());
        let collapsed = lines_text(render_exec_process_summary(test_visual(), &cell, 80));

        assert_eq!(exec_process_title(&cell), "记忆检索 · 仓库文档重整");
        assert!(
            !collapsed.contains("仓库文档重整") && !collapsed.contains("limit"),
            "标题已承载 query，折叠态参数行不得再说一遍"
        );
    }

    #[test]
    fn output_summary_keeps_first_meaningful_line_and_fold_hint() {
        assert_eq!(
            output_summary_text("\nModified files: 2\n\nUpdated files: 1"),
            "Output Modified files: 2 · 共 2 行"
        );
    }

    #[test]
    fn output_summary_handles_empty_output_without_extra_density() {
        assert_eq!(output_summary_text("\n  \n"), "Output 无有效内容");
    }

    #[test]
    fn output_detail_uses_shared_markdown_renderer() {
        let lines = render_exec_output_detail(
            test_visual(),
            "# 结论\n\n- 第一项\n- 第二项\n\n| 名称 | 状态 |\n| --- | --- |\n| axon | 已完成 |",
            100,
            Style::default(),
        );
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(text.contains("结论"));
        assert!(text.contains("第一项"));
        assert!(!text.contains("| --- | --- |"));
    }

    #[test]
    fn tool_argument_summary_uses_dedicated_input_color() {
        let cell = AgentRenderExecCell::running_with_id(
            "exec:test:tool-input-color",
            "builtin.shell_command",
            "tool-1",
            Some(r#"{"command":"cargo check --workspace"}"#.to_owned()),
        );
        let lines = render_exec_process_summary(test_visual(), &cell, 80);
        let tool_input = transcript_palette().color(TranscriptColorRole::ToolInput);

        assert!(lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
            span.content.contains("cargo check --workspace") && span.style.fg == Some(tool_input)
        }));
    }

    #[test]
    fn running_exec_does_not_render_duplicate_result_duration() {
        let mut cell = AgentRenderExecCell::running_with_id(
            "exec:test:running-duration",
            "builtin.shell_command",
            "tool-1",
            None,
        );
        cell.duration_ms = Some(6_650);

        let lines = render_exec_process_body(test_visual(), &cell, 80);
        let text = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert_eq!(text.matches("6.65s").count(), 1);
        assert!(!text.contains("✓"));
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
