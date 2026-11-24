//! MCP 管理命令交互测试。

use super::{AgentTuiState, command_handlers::AgentTuiCommandOutcome};
use crate::r#loop::agent_tui::{
    AgentTuiCatalog, AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState, AgentTuiConfigStatus,
    AgentTuiLaunchRequest, AgentTuiMcpAuthAction, AgentTuiMcpServerCatalogItem,
    AgentTuiRunOverrides, commands::MCP_USAGE, state::core_state::MCP_AUTH_NOTICE_TITLE,
};
use crate::surface::{
    cell::RenderCellData,
    cell_event_data::{AgentRenderLifecycleStatus, AgentRenderMcpCellSource},
};

#[test]
fn mcp_command_renders_dedicated_inventory_cell() {
    let mut state = test_state();
    state.header.catalog.mcp_entries =
        vec!["filesystem/read".to_owned(), "memory/search".to_owned()];

    assert!(matches!(
        state.handle_command("/mcp"),
        AgentTuiCommandOutcome::Handled
    ));

    let cell = latest_mcp_cell(&state);
    assert_eq!(cell.title, "MCP");
    assert_eq!(cell.source, AgentRenderMcpCellSource::Catalog);
    assert_eq!(cell.items.len(), 1);
    assert_eq!(cell.items[0].server, "MCP catalog");
    assert_eq!(cell.items[0].status, AgentRenderLifecycleStatus::Running);
}

#[test]
fn mcp_list_command_renders_inventory_and_requests_refresh() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/mcp list"),
        AgentTuiCommandOutcome::Handled
    ));

    let cell = latest_mcp_cell(&state);
    assert_eq!(cell.source, AgentRenderMcpCellSource::Catalog);
    assert_eq!(cell.items.len(), 1);
    assert_eq!(state.take_catalog_reload_request(), Some(true));
}

#[test]
fn mcp_command_renders_empty_inventory_state() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/mcp"),
        AgentTuiCommandOutcome::Handled
    ));

    let cell = latest_mcp_cell(&state);
    assert_eq!(cell.source, AgentRenderMcpCellSource::Catalog);
    assert_eq!(cell.items.len(), 1);
    assert_eq!(cell.items[0].server, "MCP catalog");
    assert_eq!(cell.items[0].status, AgentRenderLifecycleStatus::Running);
}

#[test]
fn mcp_command_renders_loading_inventory_state() {
    let mut state = test_state();
    state.header.catalog.mcp_state = AgentTuiCatalogLoadState::Loading;

    assert!(matches!(
        state.handle_command("/mcp"),
        AgentTuiCommandOutcome::Handled
    ));

    let cell = latest_mcp_cell(&state);
    assert_eq!(cell.source, AgentRenderMcpCellSource::Catalog);
    assert_eq!(cell.items.len(), 1);
    assert_eq!(cell.items[0].server, "MCP catalog");
    assert_eq!(cell.items[0].status, AgentRenderLifecycleStatus::Running);
}

#[test]
fn mcp_inventory_surface_renders_server_failures_after_reload_snapshot() {
    let mut state = test_state();
    state.header.catalog.mcp_state = AgentTuiCatalogLoadState::Failed;
    state.header.catalog.mcp_servers = vec![
        AgentTuiMcpServerCatalogItem::loaded(
            "gitnexus",
            vec!["query".to_owned(), "impact".to_owned()],
        ),
        AgentTuiMcpServerCatalogItem::failed("broken", "启动失败"),
    ];

    state.push_mcp_inventory();

    let cell = latest_mcp_cell(&state);
    assert_eq!(cell.source, AgentRenderMcpCellSource::Catalog);
    assert_eq!(cell.items.len(), 3);
    assert!(cell.items.iter().any(|item| {
        item.server == "broken"
            && item.status == AgentRenderLifecycleStatus::Failed
            && item.summary.as_deref() == Some("启动失败")
    }));
}

#[test]
fn mcp_command_retries_failed_catalog_before_rendering_inventory() {
    let mut state = test_state();
    state.header.catalog.mcp_state = AgentTuiCatalogLoadState::Loaded;
    state.header.catalog.mcp_servers = vec![
        AgentTuiMcpServerCatalogItem::loaded("gitnexus", vec!["query".to_owned()]),
        AgentTuiMcpServerCatalogItem::failed("morph-mcp", "启动失败"),
    ];

    assert!(matches!(
        state.handle_command("/mcp"),
        AgentTuiCommandOutcome::Handled
    ));

    let cell = latest_mcp_cell(&state);
    assert_eq!(cell.items.len(), 1);
    assert_eq!(cell.items[0].server, "MCP catalog");
    assert_eq!(cell.items[0].status, AgentRenderLifecycleStatus::Running);
    assert_eq!(state.take_catalog_reload_request(), Some(true));
}

#[test]
fn mcp_invalid_args_show_usage() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/mcp full"),
        AgentTuiCommandOutcome::Handled
    ));

    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("无效 MCP 参数应生成用法错误");
    };
    assert_eq!(cell.message, MCP_USAGE);
    assert!(state.take_mcp_auth_request().is_none());
}

#[test]
fn mcp_login_registers_background_auth_request() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/mcp login sm_cloud_platform"),
        AgentTuiCommandOutcome::Handled
    ));

    let request = state
        .take_mcp_auth_request()
        .expect("合法 login 参数应登记后台授权请求");
    assert_eq!(request.action, AgentTuiMcpAuthAction::Login);
    assert_eq!(request.server_id, "sm_cloud_platform");
    assert!(state.take_mcp_auth_request().is_none());
}

#[test]
fn mcp_logout_registers_background_auth_request() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/mcp logout gitnexus"),
        AgentTuiCommandOutcome::Handled
    ));

    let request = state
        .take_mcp_auth_request()
        .expect("合法 logout 参数应登记后台登出请求");
    assert_eq!(request.action, AgentTuiMcpAuthAction::Logout);
    assert_eq!(request.server_id, "gitnexus");
}

#[test]
fn mcp_auth_without_server_shows_usage() {
    let mut state = test_state();

    assert!(matches!(
        state.handle_command("/mcp login"),
        AgentTuiCommandOutcome::Handled
    ));

    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("缺少 server 参数应生成用法错误");
    };
    assert_eq!(cell.message, MCP_USAGE);
    assert!(state.take_mcp_auth_request().is_none());
}

#[test]
fn mcp_auth_rejects_unknown_server_from_catalog() {
    let mut state = test_state();
    state.header.catalog.mcp_servers =
        vec![AgentTuiMcpServerCatalogItem::failed("broken", "启动失败")];

    assert!(matches!(
        state.handle_command("/mcp login gitnexus"),
        AgentTuiCommandOutcome::Handled
    ));

    let Some(RenderCellData::Error(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("catalog 中不存在的 server 应生成错误");
    };
    assert_eq!(cell.title, "MCP server 未找到");
    assert!(state.take_mcp_auth_request().is_none());
}

#[test]
fn catalog_snapshot_notices_mcp_oauth_login_once() {
    let mut state = test_state();

    state.apply_catalog_load_snapshot(oauth_required_snapshot());

    assert!(state.bridge.mcp_auth_notice_shown);
    let notices = mcp_auth_notice_items(&state);
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("/mcp login sm_cloud_platform"));

    state.apply_catalog_load_snapshot(oauth_required_snapshot());

    assert_eq!(mcp_auth_notice_items(&state).len(), 1);
}

#[test]
fn catalog_snapshot_skips_mcp_auth_notice_without_oauth_failure() {
    let mut state = test_state();

    state.apply_catalog_load_snapshot(catalog_snapshot(vec![
        AgentTuiMcpServerCatalogItem::failed("broken", "启动失败"),
    ]));

    assert!(!state.bridge.mcp_auth_notice_shown);
    assert!(mcp_auth_notice_items(&state).is_empty());
}

fn oauth_required_snapshot() -> AgentTuiCatalogLoadSnapshot {
    catalog_snapshot(vec![AgentTuiMcpServerCatalogItem::failed(
        "sm_cloud_platform",
        axon_agent_runtime::MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON,
    )])
}

fn catalog_snapshot(mcp_servers: Vec<AgentTuiMcpServerCatalogItem>) -> AgentTuiCatalogLoadSnapshot {
    let mcp_state = if mcp_servers.is_empty() {
        AgentTuiCatalogLoadState::Loaded
    } else {
        AgentTuiCatalogLoadState::Failed
    };
    AgentTuiCatalogLoadSnapshot {
        skills: Vec::new(),
        skill_state: AgentTuiCatalogLoadState::Loaded,
        skill_errors: Vec::new(),
        mcp_servers,
        mcp_state,
        sessions: Vec::new(),
    }
}

fn mcp_auth_notice_items(state: &AgentTuiState) -> Vec<String> {
    state
        .cells
        .transcript
        .iter()
        .filter_map(|line| line.data.as_ref())
        .filter_map(|data| match data {
            RenderCellData::Status(cell) if cell.title == MCP_AUTH_NOTICE_TITLE => {
                Some(cell.items.as_slice())
            }
            _ => None,
        })
        .flatten()
        .map(|item| item.summary.clone())
        .collect()
}

fn latest_mcp_cell(state: &AgentTuiState) -> &crate::surface::cell_event_data::AgentRenderMcpCell {
    let Some(RenderCellData::Mcp(cell)) = state
        .cells
        .transcript
        .last()
        .and_then(|line| line.data.as_ref())
    else {
        panic!("mcp 应生成结构化 MCP 列表");
    };
    cell
}

fn test_state() -> AgentTuiState {
    AgentTuiState::new_for_test(&AgentTuiLaunchRequest {
        cwd: axon_test_support::test_temp_root(),
        prompt: None,
        workspace_instruction_paths: Vec::new(),
        config_status: AgentTuiConfigStatus::new("fixture", "model", "已认证"),
        overrides: AgentTuiRunOverrides::default(),
        catalog: AgentTuiCatalog::default(),
        runtime_tmp_dir: axon_test_support::test_temp_root().join("axon-data-tmp"),
    })
}
