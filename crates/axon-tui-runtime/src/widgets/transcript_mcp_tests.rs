use super::*;
use crate::surface::{
    cell::RenderCellRecord, cell_event_data::AgentRenderMcpCellSource, render_policy::RenderPolicy,
};

#[test]
fn mcp_items_hide_loaded_status_noise_and_keep_tool_path() {
    let visual = test_visual();
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:hide-loaded-status".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![
            mcp_item(
                "gitnexus",
                Some("query"),
                AgentRenderLifecycleStatus::Completed,
                None,
                Some(r#"{"q":"auth"}"#),
                Some("2 条结果"),
            ),
            mcp_item(
                "openviking",
                None,
                AgentRenderLifecycleStatus::Running,
                None,
                None,
                None,
            ),
        ],
    });
    let lines = render_standalone(visual, mcp_decision(&cell), false, Some(&cell), 80);
    let text = lines_text(lines);

    assert!(text.contains("MCP · gitnexus.query"));
    assert!(text.contains("参数 {\"q\":\"auth\"}"));
    assert!(!text.contains("gitnexus/query"));
    assert!(text.contains("MCP · openviking"));
    assert!(!text.contains("openviking({})"));
}

#[test]
fn mcp_items_use_status_prefix_colors() {
    let visual = test_visual();
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:status-prefix-colors".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![
            mcp_item(
                "gitnexus",
                Some("query"),
                AgentRenderLifecycleStatus::Completed,
                None,
                None,
                None,
            ),
            mcp_item(
                "openviking",
                Some("search"),
                AgentRenderLifecycleStatus::Running,
                None,
                None,
                None,
            ),
            mcp_item(
                "shell",
                Some("command"),
                AgentRenderLifecycleStatus::Failed,
                None,
                None,
                None,
            ),
        ],
    });
    let lines = render_standalone(visual, mcp_decision(&cell), true, Some(&cell), 80);

    assert_eq!(lines[0].spans[0].content.as_ref(), MCP_PREFIX_COMPLETED);
    assert_eq!(
        lines[0].spans[0].style.fg,
        Some(transcript_palette().color(TranscriptColorRole::ToolCompleted))
    );
    assert!(
        lines[0].spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(lines[1].spans[0].content.as_ref(), MCP_PREFIX_RUNNING);
    assert_eq!(
        lines[1].spans[0].style.fg,
        Some(transcript_palette().color(TranscriptColorRole::Loading))
    );
    assert_eq!(lines[2].spans[0].content.as_ref(), MCP_PREFIX_FAILED);
    assert_eq!(
        lines[2].spans[0].style.fg,
        Some(transcript_palette().color(TranscriptColorRole::ToolFailed))
    );
}

#[test]
fn failed_mcp_target_and_reason_use_error_color_and_detail_indent() {
    let mut failed_item = mcp_item(
        "gitnexus",
        Some("query"),
        AgentRenderLifecycleStatus::Failed,
        Some("call-failed"),
        None,
        None,
    );
    failed_item.summary = Some("连接被拒绝".to_owned());
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:failed-style".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![failed_item],
    });

    let lines = render_standalone(test_visual(), mcp_decision(&cell), false, Some(&cell), 120);
    let error_color = transcript_palette().color(TranscriptColorRole::ToolFailed);
    let target = lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "gitnexus.query")
        .expect("失败 MCP 应保留 server.tool 标识");

    assert_eq!(target.style.fg, Some(error_color));
    assert_eq!(lines[1].spans[0].content.as_ref(), MCP_DETAIL_PREFIX);
    assert_eq!(lines[1].spans[0].style.fg, Some(error_color));
    assert_eq!(lines[1].spans[1].style.fg, Some(error_color));
    assert!(lines_text(lines).contains("连接被拒绝"));
}

#[test]
fn failed_mcp_catalog_server_and_reason_use_error_color() {
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:failed-catalog-style".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::Catalog,
        items: vec![AgentRenderMcpItem::catalog(
            "gitnexus",
            None,
            AgentRenderLifecycleStatus::Failed,
            Some("配置文件加载失败".to_owned()),
        )],
    });

    let lines = render_standalone(test_visual(), mcp_decision(&cell), false, Some(&cell), 120);
    let error_color = transcript_palette().color(TranscriptColorRole::ToolFailed);
    let server = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "gitnexus")
        .expect("失败 catalog 应显示 server");
    let reason = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref().contains("配置文件加载失败"))
        .expect("失败 catalog 应显示原因");

    assert_eq!(server.style.fg, Some(error_color));
    assert_eq!(reason.style.fg, Some(error_color));
}

#[test]
fn mcp_server_tool_identifier_uses_dedicated_soft_teal() {
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:identifier-color".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![mcp_item(
            "gitnexus",
            Some("detect_changes"),
            AgentRenderLifecycleStatus::Completed,
            None,
            None,
            None,
        )],
    });
    let lines = render_standalone(test_visual(), mcp_decision(&cell), false, Some(&cell), 80);
    let identifier = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.as_ref() == "gitnexus.detect_changes")
        .expect("MCP server/tool 标识应保持独立 span");

    assert_eq!(
        identifier.style.fg,
        Some(transcript_palette().color(TranscriptColorRole::McpIdentifier))
    );
}

#[test]
fn single_mcp_call_renders_target_in_standalone_header() {
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:single-call-header".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![mcp_item(
            "gitnexus",
            Some("query"),
            AgentRenderLifecycleStatus::Running,
            None,
            None,
            None,
        )],
    });
    let text = lines_text(render_standalone(
        test_visual(),
        mcp_decision(&cell),
        false,
        Some(&cell),
        80,
    ));

    assert!(text.contains("MCP · gitnexus.query"));
    assert!(!text.contains("gitnexus/query"));
    assert!(!text.contains("MCP · 1 项"));
}

#[test]
fn mcp_standalone_uses_self_contained_call_cell_without_outer_primary_line() {
    let visual = test_visual();
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:self-contained-call".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![mcp_item(
            "gitnexus",
            Some("query"),
            AgentRenderLifecycleStatus::Running,
            Some("call-1"),
            Some(r#"{"q":"auth"}"#),
            None,
        )],
    });

    let text = lines_text(render_standalone(
        visual,
        mcp_decision(&cell),
        false,
        Some(&cell),
        80,
    ));

    assert!(text.contains("◌ MCP · gitnexus.query"));
    assert!(text.contains(r#"参数 {"q":"auth"}"#));
    assert!(!text.starts_with("• gitnexus.query"));
    assert!(!text.contains("mcp__gitnexus__query"));
}

#[test]
fn mcp_call_renders_invocation_and_result_detail_like_codex() {
    let visual = test_visual();
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:call-result-detail".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![mcp_item(
            "gitnexus",
            Some("query"),
            AgentRenderLifecycleStatus::Completed,
            Some("call-mcp"),
            Some(r#"{"q":"auth"}"#),
            Some("2 条结果"),
        )],
    });

    let lines = render_standalone(visual, mcp_decision(&cell), false, Some(&cell), 80);
    let text = lines_text(lines);

    assert!(text.contains("MCP · gitnexus.query"));
    assert!(text.contains(r#"参数 {"q":"auth"}"#));
    assert!(text.contains("  └ 2 条结果"));
    assert!(!text.contains("完成"));
}

#[test]
fn mcp_catalog_inventory_renders_codex_style_server_groups() {
    let visual = test_visual();
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:catalog-inventory".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "当前没有已载入的 MCP server/tool".to_owned(),
        source: AgentRenderMcpCellSource::Catalog,
        items: vec![
            AgentRenderMcpItem::catalog(
                "gitnexus",
                Some("query".to_owned()),
                AgentRenderLifecycleStatus::Completed,
                None,
            ),
            AgentRenderMcpItem::catalog(
                "gitnexus",
                Some("impact".to_owned()),
                AgentRenderLifecycleStatus::Completed,
                None,
            ),
        ],
    });

    let text = lines_text(render_standalone(
        visual,
        mcp_decision(&cell),
        false,
        Some(&cell),
        80,
    ));

    assert!(text.contains("/mcp"));
    assert!(text.contains("MCP Tools"));
    assert!(text.contains("gitnexus"));
    assert!(text.contains("Tools: impact, query"));
    assert!(!text.contains("Auth:"));
    assert!(!text.contains("个已加载"));
    assert!(!text.contains("已调用 gitnexus/query"));
}

fn mcp_item(
    server: &str,
    tool: Option<&str>,
    status: AgentRenderLifecycleStatus,
    provider_call_id: Option<&str>,
    arguments_preview: Option<&str>,
    output_preview: Option<&str>,
) -> AgentRenderMcpItem {
    let mut item = AgentRenderMcpItem::catalog(server, tool.map(ToOwned::to_owned), status, None);
    item.provider_call_id = provider_call_id.map(ToOwned::to_owned);
    item.arguments_preview = arguments_preview.map(ToOwned::to_owned);
    item.output_preview = output_preview.map(ToOwned::to_owned);
    item
}

fn test_visual() -> CellVisual {
    CellVisual {
        title_style: ratatui::style::Style::default(),
        action_text_style: ratatui::style::Style::default(),
        detail_prefix_style: ratatui::style::Style::default(),
        detail_text_style: ratatui::style::Style::default(),
    }
}

fn lines_text(lines: Vec<Line<'static>>) -> String {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>()
}

fn mcp_decision(cell: &RenderCellData) -> crate::surface::render_policy::RenderPolicyDecision {
    let RenderCellData::Mcp(cell) = cell else {
        panic!("测试仅接受 MCP cell");
    };
    RenderPolicy::decide(&RenderCellRecord::mcp_with_data(cell.clone()))
}
