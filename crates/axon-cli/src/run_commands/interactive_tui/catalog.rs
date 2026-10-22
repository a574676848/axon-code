use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    thread,
};

use axon_agent_runtime::MCP_TOOL_NAMESPACE_PREFIX;
use axon_config::ConfigLoader;
use axon_tui_runtime::{
    AgentTuiCatalog, AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState,
    AgentTuiMcpServerCatalogItem, AgentTuiModelCatalogItem, AgentTuiSessionCatalogItem,
};

use crate::management_commands::resolved_config_path;
use crate::run_commands::{AxonCli, run_session_store::resolved_axon_root};

const TUI_CATALOG_LIMIT: usize = 12;

pub(super) fn resolve_agent_tui_catalog(cli: &AxonCli) -> AgentTuiCatalog {
    let Ok(path) = resolved_config_path(cli.global.config.as_deref()) else {
        return AgentTuiCatalog::default();
    };
    let Ok(loaded) = ConfigLoader::load_from_path(&path) else {
        return AgentTuiCatalog::default();
    };
    let mut seen_names = BTreeSet::new();
    AgentTuiCatalog {
        models: loaded
            .config
            .models
            .iter()
            .filter(|model| loaded.config.model_is_enabled(model))
            .filter(|model| !model.supports_embedding)
            .filter(|model| seen_names.insert(model.name.clone()))
            .map(|model| {
                AgentTuiModelCatalogItem::new_with_runtime_defaults(
                    model.name.clone(),
                    model.id.clone(),
                    model.provider.clone(),
                    (model.context_window > 0).then_some(model.context_window),
                    None,
                )
                .with_supports_vision(model.supports_vision)
            })
            .collect(),
        providers: loaded
            .config
            .providers
            .iter()
            .filter(|provider| provider.enabled)
            .map(|provider| provider.name.clone())
            .collect(),
        skills: Vec::new(),
        skill_state: AgentTuiCatalogLoadState::Loading,
        skill_errors: Vec::new(),
        disabled_skills: loaded.config.skills.disabled.clone(),
        connectors: Vec::new(),
        mcp_servers: Vec::new(),
        mcp_state: AgentTuiCatalogLoadState::Loading,
        mcp_entries: Vec::new(),
        sessions: load_tui_session_items(&path),
    }
}

pub(crate) fn load_tui_catalog_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
    force_mcp_refresh: bool,
) -> AgentTuiCatalogLoadSnapshot {
    let workspace_cwd = crate::runtime_tools::resolve_workspace_cwd(config_path, cwd)
        .ok()
        .flatten();
    let skill_config_path = config_path.to_path_buf();
    let skill_cwd = workspace_cwd.clone();
    let mcp_config_path = config_path.to_path_buf();
    let mcp_cwd = workspace_cwd;
    let mcp_force_refresh = force_mcp_refresh;
    let skill_handle =
        thread::spawn(move || load_tui_skill_snapshot(&skill_config_path, skill_cwd.as_deref()));
    let mcp_handle = thread::spawn(move || {
        load_tui_mcp_snapshot(&mcp_config_path, mcp_cwd.as_deref(), mcp_force_refresh)
    });
    let (skills, skill_errors, skill_state) = skill_handle
        .join()
        .unwrap_or_else(|_| failed_skill_snapshot());
    let (mcp_servers, mcp_state) = mcp_handle.join().unwrap_or_else(|_| failed_mcp_snapshot());
    AgentTuiCatalogLoadSnapshot {
        skills,
        skill_state,
        skill_errors,
        mcp_servers,
        mcp_state,
        sessions: load_tui_session_items(config_path),
    }
}

fn failed_skill_snapshot() -> (Vec<String>, Vec<String>, AgentTuiCatalogLoadState) {
    (
        Vec::new(),
        vec!["skill catalog 后台加载异常退出".to_owned()],
        AgentTuiCatalogLoadState::Failed,
    )
}

fn failed_mcp_snapshot() -> (Vec<AgentTuiMcpServerCatalogItem>, AgentTuiCatalogLoadState) {
    (Vec::new(), AgentTuiCatalogLoadState::Failed)
}

fn load_tui_skill_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
) -> (Vec<String>, Vec<String>, AgentTuiCatalogLoadState) {
    let Ok(manager) = crate::runtime_tools::build_runtime_skills_manager(config_path, cwd) else {
        return (
            Vec::new(),
            vec!["skill manager 构建失败".to_owned()],
            AgentTuiCatalogLoadState::Failed,
        );
    };
    let outcome = manager.load();
    let mut names = outcome
        .skills
        .into_iter()
        .map(|skill| skill.metadata.name)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    let errors = outcome
        .errors
        .into_iter()
        .map(|error| {
            format!(
                "{}: {}",
                error.path.to_string_lossy().replace('\\', "/"),
                error.message
            )
        })
        .collect::<Vec<_>>();
    let state = if errors.is_empty() {
        AgentTuiCatalogLoadState::Loaded
    } else {
        AgentTuiCatalogLoadState::Failed
    };
    (names, errors, state)
}

fn load_tui_mcp_server_names(config_path: &Path, cwd: Option<&Path>) -> Vec<String> {
    let Ok(root) = resolved_axon_root(config_path) else {
        return Vec::new();
    };
    let home = crate::runtime_tools::runtime_home();
    let Ok(discovered) =
        axon_mcp_runtime::discover_runtime_mcp_servers(&root, cwd, home.as_deref())
    else {
        return Vec::new();
    };
    discovered.keys().cloned().collect()
}

fn load_tui_mcp_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
    force_mcp_refresh: bool,
) -> (Vec<AgentTuiMcpServerCatalogItem>, AgentTuiCatalogLoadState) {
    let server_names = load_tui_mcp_server_names(config_path, cwd);
    let snapshot = match crate::runtime_tools::try_build_runtime_tool_registry_snapshot(
        config_path,
        cwd,
        force_mcp_refresh,
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            return (
                server_names
                    .into_iter()
                    .map(|server| AgentTuiMcpServerCatalogItem::failed(server, error.clone()))
                    .collect(),
                AgentTuiCatalogLoadState::Failed,
            );
        }
    };
    let servers = mcp_server_catalog_items(server_names, snapshot);
    let state = mcp_catalog_load_state(&servers);
    (servers, state)
}

fn mcp_server_catalog_items(
    server_names: Vec<String>,
    snapshot: crate::runtime_tools::RuntimeExecutionToolSnapshot,
) -> Vec<AgentTuiMcpServerCatalogItem> {
    let mut tools_by_server = BTreeMap::<String, Vec<String>>::new();
    for tool in snapshot.registry.tools() {
        let Some(server_namespace) = tool.namespace.strip_prefix(MCP_TOOL_NAMESPACE_PREFIX) else {
            continue;
        };
        tools_by_server
            .entry(server_namespace.to_owned())
            .or_default()
            .push(tool.name.clone());
    }
    let mut failures = snapshot
        .mcp_schema_cache
        .into_iter()
        .filter_map(|report| report.failure.map(|failure| (report.server_id, failure)))
        .collect::<BTreeMap<_, _>>();

    let mut servers = Vec::new();
    for server in server_names {
        let mut tools = tools_by_server.remove(&server).unwrap_or_default();
        tools.sort();
        tools.dedup();
        if tools.is_empty() {
            let failure = failures
                .remove(&server)
                .unwrap_or_else(|| format!("MCP server {server} 未注册任何可用工具"));
            servers.push(AgentTuiMcpServerCatalogItem::failed(
                server.clone(),
                mcp_auth_failure_hint(&server, failure),
            ));
        } else {
            servers.push(AgentTuiMcpServerCatalogItem::loaded(server, tools));
        }
    }
    for (server, mut tools) in tools_by_server {
        tools.sort();
        tools.dedup();
        servers.push(AgentTuiMcpServerCatalogItem::loaded(server, tools));
    }
    servers
}

/// 为需要 OAuth 授权的 MCP 加载失败补充可执行引导。
///
/// 401/403 的失败原因由 transport 层标记，这里追加 `/mcp login` 与 `axon mcp login`
/// 两种入口，让 `/mcp` 列表直接给出下一步操作。
fn mcp_auth_failure_hint(server_id: &str, failure: String) -> String {
    if !axon_agent_runtime::mcp_oauth_required(&failure) {
        return failure;
    }
    format!(
        "{failure}；{}",
        axon_agent_runtime::mcp_oauth_login_guidance(server_id)
    )
}

fn mcp_catalog_load_state(servers: &[AgentTuiMcpServerCatalogItem]) -> AgentTuiCatalogLoadState {
    let has_loaded_tools = servers.iter().any(|server| !server.tools.is_empty());
    let has_failed_server = servers
        .iter()
        .any(|server| server.status == AgentTuiCatalogLoadState::Failed);
    if has_failed_server && !has_loaded_tools {
        AgentTuiCatalogLoadState::Failed
    } else {
        AgentTuiCatalogLoadState::Loaded
    }
}

fn load_tui_session_items(config_path: &Path) -> Vec<AgentTuiSessionCatalogItem> {
    let Ok(root) = resolved_axon_root(config_path) else {
        return Vec::new();
    };
    let service = axon_session::ThreadService::new(axon_session::JsonlThreadEventStore);
    service
        .list_threads(&root, TUI_CATALOG_LIMIT)
        .unwrap_or_default()
        .into_iter()
        .map(|thread| {
            AgentTuiSessionCatalogItem::new(
                thread.thread_id,
                thread.name,
                thread.provider,
                thread.model,
                thread.updated_at_ms,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use axon_agent_runtime::ToolRegistry;
    use axon_mcp_runtime::RuntimeMcpServerLoadReport;

    use crate::runtime_tools::RuntimeExecutionToolSnapshot;

    const HOME_WORKSPACE_SKILL: &str = "home-workspace-skill-must-not-load";
    const HOME_WORKSPACE_MCP: &str = "home_workspace_mcp_must_not_load";

    #[test]
    fn home_child_does_not_contribute_workspace_skills_or_mcp_servers() {
        let root = axon_test_support::test_temp_root().join(format!(
            "axon-tui-catalog-home-{}-{}",
            std::process::id(),
            axon_time::unix_now_ms()
        ));
        let home = root.join("home");
        let home_child = home.join("data");
        let config_path = home.join("config.toml");
        let skill_dir = home_child
            .join(".axon")
            .join("skills")
            .join(HOME_WORKSPACE_SKILL);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {HOME_WORKSPACE_SKILL}\ndescription: Home boundary test\n---\n"),
        )
        .unwrap();
        fs::write(
            home_child.join(".mcp.json"),
            format!(
                r#"{{"mcpServers":{{"{HOME_WORKSPACE_MCP}":{{"command":"missing-home-mcp"}}}}}}"#
            ),
        )
        .unwrap();

        let snapshot = load_tui_catalog_snapshot(&config_path, Some(&home_child), false);

        assert!(
            snapshot
                .skills
                .iter()
                .all(|name| name != HOME_WORKSPACE_SKILL)
        );
        assert!(
            snapshot
                .mcp_servers
                .iter()
                .all(|server| server.name != HOME_WORKSPACE_MCP)
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mcp_catalog_preserves_server_load_failure_reason() {
        let failure = "MCP schema 注册失败：tool description 不能为空";
        let servers = mcp_server_catalog_items(
            vec!["gitnexus".to_owned()],
            RuntimeExecutionToolSnapshot {
                registry: ToolRegistry::new(),
                mcp_schema_cache: vec![RuntimeMcpServerLoadReport {
                    server_id: "gitnexus".to_owned(),
                    source: axon_mcp_runtime::McpServerSource::User,
                    disposition: "miss",
                    failure: Some(failure.to_owned()),
                }],
            },
        );

        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].error.as_deref(), Some(failure));
    }

    #[test]
    fn mcp_catalog_state_keeps_partial_server_failures_non_blocking() {
        let servers = vec![
            AgentTuiMcpServerCatalogItem::loaded("gitnexus", vec!["query".to_owned()]),
            AgentTuiMcpServerCatalogItem::failed("morph-mcp", "启动失败"),
        ];

        assert_eq!(
            mcp_catalog_load_state(&servers),
            AgentTuiCatalogLoadState::Loaded
        );
    }

    #[test]
    fn mcp_catalog_state_fails_when_no_server_loaded_tools() {
        let servers = vec![AgentTuiMcpServerCatalogItem::failed(
            "morph-mcp",
            "启动失败",
        )];

        assert_eq!(
            mcp_catalog_load_state(&servers),
            AgentTuiCatalogLoadState::Failed
        );
    }

    #[test]
    fn mcp_catalog_appends_login_guidance_for_oauth_failures() {
        let failure = axon_agent_runtime::MCP_OAUTH_REQUIRED_UNAUTHORIZED_REASON.to_owned();
        let servers = mcp_server_catalog_items(
            vec!["sm_cloud_platform".to_owned()],
            RuntimeExecutionToolSnapshot {
                registry: ToolRegistry::new(),
                mcp_schema_cache: vec![RuntimeMcpServerLoadReport {
                    server_id: "sm_cloud_platform".to_owned(),
                    source: axon_mcp_runtime::McpServerSource::User,
                    disposition: "miss",
                    failure: Some(failure.clone()),
                }],
            },
        );

        assert_eq!(servers.len(), 1);
        let error = servers[0].error.as_deref().expect("应保留失败原因");
        assert!(error.starts_with(&failure));
        assert!(error.contains("/mcp login sm_cloud_platform"));
        assert!(error.contains("axon mcp login sm_cloud_platform"));
    }

    #[test]
    fn mcp_auth_failure_hint_keeps_unrelated_failures_untouched() {
        let failure = "MCP server 启动失败：program not found";

        assert_eq!(
            mcp_auth_failure_hint("gitnexus", failure.to_owned()),
            failure
        );
    }
}
