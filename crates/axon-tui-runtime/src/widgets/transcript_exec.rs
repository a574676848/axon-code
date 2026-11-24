//! Exec/tool transcript cell 渲染。

use ratatui::text::Line;

use crate::{
    surface::{
        cell::RenderCellData, cell_event_data::AgentRenderExecCell,
        render_policy::RenderPolicyDecision,
    },
    widgets::{
        transcript_cells::render_prefixed_block, transcript_exec_process,
        transcript_style::CellVisual,
    },
};

pub(super) fn render_standalone(
    visual: CellVisual,
    decision: RenderPolicyDecision,
    selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    let Some(cell) = exec_cell_data(data) else {
        return Vec::new();
    };
    let title = exec_process_title_with_hint(cell, decision, selected);
    let mut lines = render_prefixed_block(
        "",
        &title,
        visual.title_style,
        visual.action_text_style,
        width,
    );
    if decision.render_expanded(selected) {
        lines.extend(transcript_exec_process::render_exec_process_body(
            visual, cell, width,
        ));
    } else {
        lines.extend(transcript_exec_process::render_exec_process_summary(
            visual, cell, width,
        ));
    }
    lines
}

fn exec_process_title_with_hint(
    cell: &AgentRenderExecCell,
    decision: RenderPolicyDecision,
    selected: bool,
) -> String {
    let mut title = transcript_exec_process::exec_process_title(cell);
    if decision.openable {
        title.push_str(if selected {
            " · 点击收起"
        } else {
            " · 点击查看详情"
        });
    }
    title
}

fn exec_cell_data(data: Option<&RenderCellData>) -> Option<&AgentRenderExecCell> {
    match data {
        Some(RenderCellData::Exec(cell)) => Some(cell),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Modifier, Style};

    use super::render_standalone;
    use crate::surface::{
        cell::{RenderCellData, RenderCellRecord},
        cell_event_data::{AgentRenderExecCell, AgentRenderExecCompletion, AgentRenderExecStatus},
        render_policy::RenderPolicy,
    };
    use crate::widgets::transcript_style::CellVisual;

    #[test]
    fn exec_standalone_uses_tool_name_without_repeating_status() {
        let data = RenderCellData::Exec(exec_cell(AgentRenderExecStatus::Running));
        let decision = RenderPolicy::decide(&RenderCellRecord::agent_message(
            "assistant:test:exec-probe",
            "probe",
        ));
        let lines = render_standalone(test_visual(), decision, false, Some(&data), 80);
        let title = line_text(lines.first().expect("exec title line"));
        let text = lines_text(lines);

        assert!(title.contains("执行"));
        assert!(!title.contains("builtin.shell_command"));
        assert!(!title.contains("shell_command"));
        assert!(text.contains("◌ Running"));
        assert!(!text.contains("执行 Running"));
    }

    #[test]
    fn exec_titles_project_builtin_tools_to_short_labels() {
        let cases = [
            ("builtin.memory_search", "记忆检索", Some("记忆")),
            ("builtin.read_file", "读取", None),
            ("builtin.edit", "编辑", None),
            ("builtin.shell_command", "执行", None),
        ];

        for (tool_name, expected_label, expected_target) in cases {
            let data = RenderCellData::Exec(AgentRenderExecCell::completed_with_id(
                AgentRenderExecCompletion {
                    id: "exec:test:display".to_owned(),
                    tool_name: tool_name.to_owned(),
                    status: "completed".to_owned(),
                    provider_call_id: "tool-1".to_owned(),
                    summary: None,
                    arguments_preview: None,
                    output_preview: None,
                    duration_ms: None,
                },
            ));
            let decision = RenderPolicy::decide(&RenderCellRecord::agent_message(
                "assistant:test:exec-display",
                "probe",
            ));
            let lines = render_standalone(test_visual(), decision, false, Some(&data), 80);
            let title = line_text(lines.first().expect("exec title line"));

            assert!(title.contains(expected_label));
            if let Some(target) = expected_target {
                assert!(title.contains(target));
            }
            assert!(!title.contains("builtin_"));
            assert!(!title.contains("builtin."));
        }
    }

    #[test]
    fn long_tool_output_title_exposes_click_toggle_state() {
        let cell = AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
            id: "exec:test:click-toggle".to_owned(),
            tool_name: "builtin.shell_command".to_owned(),
            status: "completed".to_owned(),
            provider_call_id: "tool-1".to_owned(),
            summary: None,
            arguments_preview: None,
            output_preview: Some("x".repeat(300)),
            duration_ms: None,
        });
        let record = RenderCellRecord::exec_with_data(
            cell.clone(),
            crate::surface::cell::RenderCellAction::Ran,
        );
        let decision = RenderPolicy::decide(&record);
        let collapsed = render_standalone(
            test_visual(),
            decision,
            false,
            Some(&RenderCellData::Exec(cell.clone())),
            80,
        );
        let expanded = render_standalone(
            test_visual(),
            decision,
            true,
            Some(&RenderCellData::Exec(cell)),
            80,
        );

        assert!(line_text(collapsed.first().expect("collapsed title")).contains("点击查看详情"));
        let expanded_title = expanded.first().expect("expanded title");
        assert!(line_text(expanded_title).contains("点击收起"));
        assert!(
            !expanded_title.spans[0]
                .style
                .add_modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn collapsed_shell_tool_shows_command_before_status() {
        let cell = AgentRenderExecCell::running_with_id(
            "exec:test:command-summary",
            "builtin.shell_command",
            "tool-1",
            Some(
                r#"{"command":"npm install -g @larksuite/lark-cli","yield_time_ms":1000}"#
                    .to_owned(),
            ),
        );
        let record = RenderCellRecord::exec_with_data(
            cell.clone(),
            crate::surface::cell::RenderCellAction::Running,
        );
        let decision = RenderPolicy::decide(&record);
        let lines = render_standalone(
            test_visual(),
            decision,
            false,
            Some(&RenderCellData::Exec(cell)),
            80,
        );
        let text = lines_text(lines);

        assert!(text.contains("$ npm install -g @larksuite/lark-cli"));
        assert!(!text.contains("yield_time_ms"));
        assert!(text.find("$ npm").expect("command") < text.find("Running").expect("status"));
    }

    #[test]
    fn collapsed_read_tool_shows_path_instead_of_raw_json() {
        let cell = AgentRenderExecCell::completed_with_id(AgentRenderExecCompletion {
            id: "exec:test:path-summary".to_owned(),
            tool_name: "builtin.read_file".to_owned(),
            status: "completed".to_owned(),
            provider_call_id: "tool-1".to_owned(),
            summary: None,
            arguments_preview: Some(
                r#"{"path":"crates/axon-cli/src/main.rs","line_start":1}"#.to_owned(),
            ),
            output_preview: None,
            duration_ms: None,
        });
        let record = RenderCellRecord::exec_with_data(
            cell.clone(),
            crate::surface::cell::RenderCellAction::Ran,
        );
        let decision = RenderPolicy::decide(&record);
        let text = lines_text(render_standalone(
            test_visual(),
            decision,
            false,
            Some(&RenderCellData::Exec(cell)),
            80,
        ));

        assert!(text.contains("$ crates/axon-cli/src/main.rs"));
        assert!(!text.contains("line_start"));
    }

    #[test]
    fn collapsed_shell_tool_removes_runtime_shell_wrapper() {
        let cell = AgentRenderExecCell::running_with_id(
            "exec:test:shell-wrapper",
            "builtin.shell_command",
            "tool-1",
            Some(r#"{"command":"/bin/zsh -lc npm install -g @larksuite/lark-cli"}"#.to_owned()),
        );
        let record = RenderCellRecord::exec_with_data(
            cell.clone(),
            crate::surface::cell::RenderCellAction::Running,
        );
        let decision = RenderPolicy::decide(&record);
        let text = lines_text(render_standalone(
            test_visual(),
            decision,
            false,
            Some(&RenderCellData::Exec(cell)),
            80,
        ));

        assert!(text.contains("$ npm install -g @larksuite/lark-cli"));
        assert!(!text.contains("/bin/zsh -lc"));
    }

    fn exec_cell(status: AgentRenderExecStatus) -> AgentRenderExecCell {
        let mut cell = AgentRenderExecCell::running_with_id(
            "exec:test:widget",
            "builtin.shell_command",
            "tool-1",
            None,
        );
        cell.status = status;
        cell
    }

    fn test_visual() -> CellVisual {
        CellVisual {
            title_style: Style::default(),
            action_text_style: Style::default(),
            detail_prefix_style: Style::default(),
            detail_text_style: Style::default(),
        }
    }

    fn lines_text(lines: Vec<ratatui::text::Line<'static>>) -> String {
        lines
            .into_iter()
            .map(|line| line_text(&line))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn line_text(line: &ratatui::text::Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }
}
