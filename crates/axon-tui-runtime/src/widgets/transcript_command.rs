//! Command transcript cell 渲染。

use ratatui::text::Line;

use crate::{
    r#loop::agent_tui::display_units::normalize_display_units_in_text,
    surface::{
        cell::{RenderCellData, RenderStatusItem},
        visual_density::BULLET_PREFIX,
    },
    widgets::{
        transcript_cells::{
            CELL_DETAIL_LIMIT, normalize_and_truncate_cell_text, render_wrapped_block,
        },
        transcript_style::CellVisual,
    },
};

const COMMAND_PREFIX: &str = BULLET_PREFIX;
const COMMAND_EXPANDED_LIMIT: usize = usize::MAX;

pub(super) fn render_standalone(
    visual: CellVisual,
    _selected: bool,
    data: Option<&RenderCellData>,
    width: u16,
) -> Vec<Line<'static>> {
    render_command_body(visual, data, width, COMMAND_EXPANDED_LIMIT)
}

fn render_command_body(
    visual: CellVisual,
    data: Option<&RenderCellData>,
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    match data {
        Some(RenderCellData::Command(cell)) => {
            render_command_items(visual, &cell.items, width, limit)
        }
        _ => Vec::new(),
    }
}

fn render_command_items(
    visual: CellVisual,
    items: &[RenderStatusItem],
    width: u16,
    limit: usize,
) -> Vec<Line<'static>> {
    let original_count = items.len();
    let mut lines = items
        .iter()
        .take(limit)
        .flat_map(|item| render_command_item(visual, item, width))
        .collect::<Vec<_>>();
    append_omitted(&mut lines, visual, original_count, limit, width);
    lines
}

fn render_command_item(
    visual: CellVisual,
    item: &RenderStatusItem,
    width: u16,
) -> Vec<Line<'static>> {
    let text = normalize_and_truncate_cell_text(&command_item_text(item), CELL_DETAIL_LIMIT);
    render_wrapped_block(
        COMMAND_PREFIX,
        &" ".repeat(COMMAND_PREFIX.len()),
        &text,
        visual.detail_prefix_style,
        visual.detail_text_style,
        width,
    )
}

fn command_item_text(item: &RenderStatusItem) -> String {
    let mut parts = vec![command_kind_label(&item.kind).to_owned()];
    if !item.summary.is_empty() {
        parts.push(normalize_display_units_in_text(&item.summary));
    }
    parts.join(" · ")
}

fn command_kind_label(kind: &str) -> &str {
    match kind {
        "Model" | "model" => "模型",
        "Think" | "think" => "思考",
        "Session" | "session" => "会话",
        "Verify" | "verify" => "验证",
        "Tool" | "tool" => "工具",
        "Preset" | "preset" => "预设",
        "Permission" | "permission" => "权限",
        "Profile" | "profile" => "模式",
        "Policy" | "policy" => "策略",
        "Memory" | "memory" => "记忆",
        "Config" | "config" => "配置",
        "Env" | "env" => "环境",
        "Auth" | "auth" => "认证",
        "status.version" => "版本",
        "status.repository" => "仓库",
        "status.model" => "模型",
        "status.provider" => "模型供应商",
        "status.directory" => "目录",
        "status.permissions" => "权限",
        "status.agents" => "AGENTS.md",
        "status.session" => "会话",
        "status.token_usage" => "Token 用量",
        "status.context_window" => "上下文窗口",
        "cwd" => "目录",
        "git" => "Git",
        "tokens" => "上下文",
        "Skill" | "skill" => "技能",
        value => value,
    }
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
        COMMAND_PREFIX,
        &" ".repeat(COMMAND_PREFIX.len()),
        &format!("另 {omitted} 项"),
        visual.detail_prefix_style,
        visual.detail_text_style,
        width,
    ));
}

#[cfg(test)]
mod tests {
    use ratatui::{style::Style, text::Line};

    use super::render_standalone;
    use crate::widgets::transcript_style::CellVisual;
    use crate::{
        AgentTuiLifecyclePhase,
        surface::cell::{RenderCellData, RenderCommandCell, RenderStatusItem},
    };

    #[test]
    fn command_cell_renders_typed_feedback_with_localized_labels() {
        let data = RenderCellData::Command(RenderCommandCell::new(
            "test:command:localized",
            "设置",
            vec![status_item("Model", "gpt-5", Some("command"))],
        ));
        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 96));

        assert!(text.contains("模型"));
        assert!(text.contains("gpt-5"));
        assert!(!text.contains("Model"));
        assert!(!text.starts_with("设置"));
    }

    #[test]
    fn command_cell_normalizes_raw_millisecond_units() {
        let data = RenderCellData::Command(RenderCommandCell::new(
            "test:command:duration",
            "设置",
            vec![status_item("Verify", "timeout_ms=65000", Some("command"))],
        ));
        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 96));

        assert!(text.contains("验证 · timeout_ms=1m 05s"));
        assert!(!text.contains("65000ms"));
    }

    #[test]
    fn command_cell_localizes_status_command_keys() {
        let data = RenderCellData::Command(RenderCommandCell::new(
            "test:command:keys",
            "状态",
            vec![
                status_item("model", "gpt-5", Some("command")),
                status_item("think", "high", Some("command")),
                status_item("session", "new", Some("command")),
                status_item("cwd", "E:/zbg/axon-code", Some("command")),
                status_item("git", "master clean", Some("command")),
                status_item("tokens", "5.11K", Some("command")),
                status_item("status.version", "0.1.0", Some("command")),
                status_item(
                    "status.provider",
                    "local - http://127.0.0.1:5434/v1",
                    Some("command"),
                ),
                status_item(
                    "status.context_window",
                    "剩余 10%（已用 235K / 258K）",
                    Some("command"),
                ),
            ],
        ));
        let text = lines_text(render_standalone(test_visual(), false, Some(&data), 96));

        assert!(text.contains("模型 · gpt-5"));
        assert!(text.contains("思考 · high"));
        assert!(text.contains("会话 · new"));
        assert!(text.contains("目录 · E:/zbg/axon-code"));
        assert!(text.contains("Git · master clean"));
        assert!(text.contains("上下文 · 5.11K"));
        assert!(text.contains("版本 · 0.1.0"));
        assert!(text.contains("模型供应商 · local - http://127.0.0.1:5434/v1"));
        assert!(text.contains("上下文窗口 · 剩余 10%（已用 235K / 258K）"));
        assert!(!text.contains("model ·"));
        assert!(!text.contains("tokens ·"));
        assert!(!text.contains("status.provider"));
    }

    fn status_item(kind: &str, summary: &str, phase: Option<&str>) -> RenderStatusItem {
        RenderStatusItem {
            kind: kind.to_owned(),
            summary: summary.to_owned(),
            phase: phase.map(AgentTuiLifecyclePhase::from_raw),
            indicator: None,
            tool_name: None,
            duration_ms: None,
            turn_id: None,
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
            .flat_map(|line| line.spans.into_iter())
            .map(|span| span.content.into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
