//! Agent TUI 首屏 transcript。

use super::{
    AXON_VERSION, AgentTuiCatalog, AgentTuiLaunchRequest, error_projection::local_error_cell,
};
use crate::surface::cell::RenderCellRecord;

const AXON_CONFIG_WARNING_TITLE: &str = "配置需要处理";
const AXON_CONFIG_WARNING_HINT: &str = "运行 axon setup 完成模型和 API key 配置后再提交任务。";
const AXON_CONFIG_WARNING_ERROR_ID: &str = "error:startup:config";
const STARTUP_TITLE: &str = "Axon Personal Agent";
const AXON_LOGO: &str = "  ⚡\n╭────────╮\n│  <>×<> │\n│  AXON  │\n╰────────╯";
const AXON_SLOGAN_PRIMARY: &str = "知你所想，办你所托。";
const AXON_SLOGAN_SECONDARY: &str =
    "融入你的工作区与工作节奏，在每一次交互中持续深化认知的专属助理。";
const AXON_SLOGAN_EN: &str = "Knowing Your Intent, Delivering Your Outcome - A personal agent deeply woven into your workspace, evolving with every interaction.";
const AXON_GITHUB_URL: &str = "https://github.com/a574676848/axon-code";

/// 构造首屏 transcript。
pub(super) fn startup_transcript(request: &AgentTuiLaunchRequest) -> Vec<RenderCellRecord> {
    let mut lines = vec![RenderCellRecord::startup(startup_overview(request))];
    if let Some(warning) = startup_config_warning(request) {
        lines.push(RenderCellRecord::error_with_data(warning));
    }
    lines
}

fn startup_overview(request: &AgentTuiLaunchRequest) -> String {
    startup_overview_for_catalog(&request.catalog)
}

pub(super) fn startup_overview_for_catalog(catalog: &AgentTuiCatalog) -> String {
    let mut lines = vec![
        AXON_LOGO.to_owned(),
        String::new(),
        format!("{STARTUP_TITLE} v{AXON_VERSION}"),
        AXON_SLOGAN_PRIMARY.to_owned(),
        AXON_SLOGAN_SECONDARY.to_owned(),
        AXON_SLOGAN_EN.to_owned(),
        String::new(),
        format!(
            "入口已就绪 · MCP {} · Skill {}",
            catalog.mcp_summary_label(),
            catalog.skill_summary_label()
        ),
    ];
    lines.push(String::new());
    lines.push(format!("Tip: 访问 Axon GitHub：{AXON_GITHUB_URL}"));
    lines.join("\n")
}

fn startup_config_warning(
    request: &AgentTuiLaunchRequest,
) -> Option<crate::surface::cell_event_data::AgentRenderErrorCell> {
    let status = request.config_status.api_key_status.trim();
    if config_status_ready(status) {
        return None;
    }
    Some(local_error_cell(
        AXON_CONFIG_WARNING_ERROR_ID,
        AXON_CONFIG_WARNING_TITLE,
        status,
        Some(AXON_CONFIG_WARNING_HINT.to_owned()),
    ))
}

fn config_status_ready(status: &str) -> bool {
    status == "已认证" || status.eq_ignore_ascii_case("ok") || status.eq_ignore_ascii_case("ready")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r#loop::agent_tui::{AgentTuiCatalogLoadState, AgentTuiMcpServerCatalogItem};

    fn catalog(
        skills: &[&str],
        servers: Vec<AgentTuiMcpServerCatalogItem>,
        mcp_state: AgentTuiCatalogLoadState,
    ) -> AgentTuiCatalog {
        AgentTuiCatalog {
            skills: skills.iter().map(|skill| (*skill).to_owned()).collect(),
            mcp_servers: servers,
            mcp_state,
            ..AgentTuiCatalog::default()
        }
    }

    fn ready_line(catalog: &AgentTuiCatalog) -> String {
        startup_overview_for_catalog(catalog)
            .lines()
            .find(|line| line.starts_with("入口已就绪"))
            .expect("首屏应包含入口就绪行")
            .to_owned()
    }

    #[test]
    fn startup_line_counts_mcp_servers_instead_of_tools() {
        // 3 台 server 合计 33 个工具：MCP 读数必须是台数 3，工具条数不得顶上这个位置。
        let servers = vec![
            AgentTuiMcpServerCatalogItem::loaded("a", vec!["t".to_owned(); 11]),
            AgentTuiMcpServerCatalogItem::loaded("b", vec!["t".to_owned(); 19]),
            AgentTuiMcpServerCatalogItem::loaded("c", vec!["t".to_owned(); 3]),
        ];
        let catalog = catalog(&["s1", "s2"], servers, AgentTuiCatalogLoadState::Loaded);

        assert_eq!(ready_line(&catalog), "入口已就绪 · MCP 3 · Skill 2");
    }

    #[test]
    fn startup_line_keeps_failed_server_segment_after_loaded_count() {
        let servers = vec![
            AgentTuiMcpServerCatalogItem::loaded("a", vec!["t".to_owned()]),
            AgentTuiMcpServerCatalogItem::loaded("b", vec!["t".to_owned()]),
            AgentTuiMcpServerCatalogItem::failed("c", "启动失败"),
        ];
        let catalog = catalog(&["s1"], servers, AgentTuiCatalogLoadState::Loaded);

        assert_eq!(
            ready_line(&catalog),
            "入口已就绪 · MCP 2 · 失败 1 · Skill 1"
        );
    }

    #[test]
    fn startup_line_reports_loading_catalog_without_counts() {
        let catalog = catalog(&["s1"], Vec::new(), AgentTuiCatalogLoadState::Loading);

        assert_eq!(ready_line(&catalog), "入口已就绪 · MCP 加载中 · Skill 1");
    }
}
