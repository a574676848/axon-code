use ratatui::{style::Style, text::Line};

use crate::surface::{
    cell::{RenderCellData, RenderCellRecord},
    cell_event_data::{
        AgentRenderLifecycleStatus, AgentRenderMcpCell, AgentRenderMcpCellSource,
        AgentRenderMcpItem,
    },
    render_policy::RenderPolicy,
};

use super::{transcript_mcp::render_standalone, transcript_style::CellVisual};

#[test]
fn mcp_click_expansion_reveals_arguments_and_real_result() {
    let cell = RenderCellData::Mcp(AgentRenderMcpCell {
        id: "mcp:test:expand-real-result".to_owned(),
        title: "MCP".to_owned(),
        empty_text: "无 MCP 调用".to_owned(),
        source: AgentRenderMcpCellSource::ToolCall,
        items: vec![mcp_item(
            "gitnexus",
            Some("query"),
            AgentRenderLifecycleStatus::Completed,
            Some("call-expand"),
            Some(r#"{"q":"真实查询"}"#),
            Some("真实结果第一行\n真实结果第二行"),
        )],
    });
    let RenderCellData::Mcp(mcp_cell) = &cell else {
        unreachable!();
    };
    let decision = RenderPolicy::decide(&RenderCellRecord::mcp_with_data(mcp_cell.clone()));
    let collapsed = lines_text(render_standalone(
        test_visual(),
        decision,
        false,
        Some(&cell),
        80,
    ));
    let expanded = lines_text(render_standalone(
        test_visual(),
        decision,
        true,
        Some(&cell),
        80,
    ));

    assert!(collapsed.contains("点击查看详情"));
    assert!(collapsed.contains("参数"));
    assert!(expanded.contains("点击收起"));
    assert!(expanded.contains("参数"));
    assert!(expanded.contains("真实结果第一行"));
    assert!(expanded.contains("真实结果第二行"));
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
        title_style: Style::default(),
        action_text_style: Style::default(),
        detail_prefix_style: Style::default(),
        detail_text_style: Style::default(),
    }
}

fn lines_text(lines: Vec<Line<'static>>) -> String {
    lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect()
}
