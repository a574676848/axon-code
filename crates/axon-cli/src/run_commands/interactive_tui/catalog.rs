use std::{collections::BTreeMap, path::Path, thread};

use axon_agent_runtime::MCP_TOOL_NAMESPACE_PREFIX;
use axon_config::ConfigLoader;
use axon_tui_runtime::{
    AgentTuiCatalog, AgentTuiCatalogLoadSnapshot, AgentTuiCatalogLoadState,
    AgentTuiMcpServerCatalogItem, AgentTuiSessionCatalogItem,
};

use crate::management_commands::resolved_config_path;
use crate::run_commands::{AxonCli, run_session_store::resolved_axon_root};

use super::catalog_projection::tui_config_projection;

/// `/sessions` 默认列出的主会话条数；与会话清单默认页同源，两处条数因此一致。
const TUI_CATALOG_LIMIT: usize = crate::session_commands::SESSION_CLI_DEFAULT_LIMIT;
/// `/sessions --all` 需要看到归档会话，归档项单独取一份，避免归档项挤掉在用会话。
const TUI_ARCHIVED_CATALOG_LIMIT: usize = 8;

pub(super) fn resolve_agent_tui_catalog(cli: &AxonCli) -> AgentTuiCatalog {
    let Ok(path) = resolved_config_path(cli.global.config.as_deref()) else {
        return AgentTuiCatalog::default();
    };
    let Ok(loaded) = ConfigLoader::load_from_path(&path) else {
        return AgentTuiCatalog::default();
    };
    let projection = tui_config_projection(&loaded.config);
    AgentTuiCatalog {
        models: projection.models,
        providers: projection.providers,
        skills: Vec::new(),
        skill_state: AgentTuiCatalogLoadState::Loading,
        skill_errors: Vec::new(),
        disabled_skills: loaded.config.skills.disabled.clone(),
        connectors: Vec::new(),
        mcp_servers: Vec::new(),
        mcp_state: AgentTuiCatalogLoadState::Loading,
        mcp_entries: Vec::new(),
        sessions: load_tui_session_items(&path, cli.global.cwd.as_deref()),
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
    let session_cwd = cwd
        .or(workspace_cwd.as_deref())
        .map(std::path::Path::to_path_buf);
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
        sessions: load_tui_session_items(config_path, session_cwd.as_deref()),
        /*
         * 配置投影跟着同一次复读一起算：`/model` 与头部的强度读数都读这一份，
         * 因此改完 `config.toml` 不需要重启 TUI。读不到配置时留空，头部保留现状。
         */
        config: ConfigLoader::load_from_path(config_path)
            .ok()
            .map(|loaded| tui_config_projection(&loaded.config)),
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

/// 读取用户可见的会话目录。
///
/// 唯一事实源是 `sessions(kind='primary')`：thread 是所有轮次（自迭代、目标执行、
/// 子代理）的镜像，按 thread 目录列会话会把内部线程混进用户列表，归档口径也与
/// 会话表分叉。列表按工作区收敛：与桌面端一样只列当前工作区的会话，换一个目录启动
/// 就看到那个目录自己的会话，而不是全机所有工作区的混排。
fn load_tui_session_items(
    config_path: &Path,
    cwd: Option<&Path>,
) -> Vec<AgentTuiSessionCatalogItem> {
    // 空目录与「库打不开」必须留痕：后者（例如版本不兼容）会让会话面板看起来像「一条
    // 会话都没有」，而真实原因是运行态库需要重建。
    let Ok(store) = crate::run_commands::run_agent_store::shared_runtime_store(config_path) else {
        axon_logger::warning("TUI 会话目录读取失败：运行态库不可用");
        return Vec::new();
    };
    let Ok(rows) = crate::sessions_http_adapter::read_session_rows(&store) else {
        axon_logger::warning("TUI 会话目录读取失败：会话表不可读（可能是运行态库版本不兼容）");
        return Vec::new();
    };
    session_catalog_items(rows, workspace_id_for_cwd(cwd))
}

/// 当前工作区标识；没有工作目录或解析失败时为空（此时不按工作区收敛）。
fn workspace_id_for_cwd(cwd: Option<&Path>) -> Option<String> {
    let cwd = cwd?;
    axon_session::WorkspaceResolver
        .resolve(cwd, axon_time::unix_now_ms())
        .ok()
        .map(|workspace| workspace.workspace_id)
}

/// 会话清单行到 TUI 目录的投影：默认只留一页在用会话，归档项另收一页。
///
/// 与会话清单读同一批行，条数因此只有「一页」这一种口径，不再按两个不同上限拼凑。
fn session_catalog_items(
    rows: Vec<crate::sessions_http_adapter::SessionListRow>,
    workspace_id: Option<String>,
) -> Vec<AgentTuiSessionCatalogItem> {
    let scoped_rows = match workspace_id.as_deref() {
        Some(workspace_id) => rows
            .into_iter()
            .filter(|row| row.workspace_id.as_deref() == Some(workspace_id))
            .collect::<Vec<_>>(),
        None => rows,
    };
    let mut active = Vec::new();
    let mut archived = Vec::new();
    for row in scoped_rows {
        let item = session_catalog_item(row);
        if item.archived {
            if archived.len() < TUI_ARCHIVED_CATALOG_LIMIT {
                archived.push(item);
            }
        } else if active.len() < TUI_CATALOG_LIMIT {
            active.push(item);
        }
        if active.len() >= TUI_CATALOG_LIMIT && archived.len() >= TUI_ARCHIVED_CATALOG_LIMIT {
            break;
        }
    }
    active.extend(archived);
    active
}

/// 会话清单行到 TUI 目录项的投影：标题、来源与归档位都来自会话表本身。
///
/// 展示优先用用户当初选择的模型别名，别名缺失时才回落到上游模型 id。
fn session_catalog_item(
    row: crate::sessions_http_adapter::SessionListRow,
) -> AgentTuiSessionCatalogItem {
    let model = row.model_name.clone().unwrap_or_else(|| row.model.clone());
    AgentTuiSessionCatalogItem::new(
        row.session_id,
        Some(row.title),
        row.provider,
        model,
        row.last_activity_at_ms,
    )
    .with_archived(row.archived)
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

    /// 会话清单行：只填与目录投影相关的列，其余取默认值。
    fn session_row(
        session_id: &str,
        provider: &str,
        model: &str,
        model_name: Option<&str>,
        archived: bool,
    ) -> crate::sessions_http_adapter::SessionListRow {
        crate::sessions_http_adapter::SessionListRow {
            session_id: session_id.to_owned(),
            title: format!("{session_id} 标题"),
            status: "idle".to_owned(),
            pinned: false,
            archived,
            workspace_id: None,
            last_activity_at_ms: 10,
            turn_count: 1,
            total_active_ms: 0,
            ttft_p50_ms: None,
            ttft_max_ms: 0,
            open_job_count: 0,
            channel: "tui".to_owned(),
            destination: String::new(),
            last_channel: "tui".to_owned(),
            audience: "私密".to_owned(),
            interaction_count: 1,
            provider: provider.to_owned(),
            model: model.to_owned(),
            model_name: model_name.map(str::to_owned),
            running: false,
            pending_question_count: 0,
        }
    }

    #[test]
    fn session_catalog_items_use_session_rows_and_split_archived_bucket() {
        let items = session_catalog_items(
            vec![
                session_row("s-active", "openai", "gpt-5.5-id", Some("gpt-5.5"), false),
                session_row("s-archived", "anthropic", "claude-4", None, true),
            ],
            None,
        );

        assert_eq!(
            items
                .iter()
                .map(|item| item.session_id.as_str())
                .collect::<Vec<_>>(),
            ["s-active", "s-archived"],
            "在用会话在前，归档项在后"
        );
        assert!(!items[0].archived);
        assert!(items[1].archived);
        // 展示优先用用户选择的别名，没有别名才回落到上游模型 id。
        assert_eq!(items[0].provider, "openai");
        assert_eq!(items[0].model, "gpt-5.5");
        assert_eq!(items[1].model, "claude-4");
    }

    #[test]
    fn session_catalog_items_cap_active_and_archived_pages() {
        let mut rows = (0..TUI_CATALOG_LIMIT + 5)
            .map(|index| session_row(&format!("active-{index}"), "openai", "gpt-5.5", None, false))
            .collect::<Vec<_>>();
        rows.extend((0..TUI_ARCHIVED_CATALOG_LIMIT + 3).map(|index| {
            session_row(
                &format!("archived-{index}"),
                "openai",
                "gpt-5.5",
                None,
                true,
            )
        }));

        let items = session_catalog_items(rows, None);

        assert_eq!(items.len(), TUI_CATALOG_LIMIT + TUI_ARCHIVED_CATALOG_LIMIT);
        assert_eq!(
            items.iter().filter(|item| item.archived).count(),
            TUI_ARCHIVED_CATALOG_LIMIT
        );
        assert_eq!(
            items.iter().filter(|item| !item.archived).count(),
            TUI_CATALOG_LIMIT
        );
    }

    #[test]
    fn session_catalog_items_keep_only_the_current_workspace() {
        let mut mine = session_row("s-mine", "openai", "gpt-5.5", None, false);
        mine.workspace_id = Some("workspace-current".to_owned());
        let mut other = session_row("s-other", "openai", "gpt-5.5", None, false);
        other.workspace_id = Some("workspace-elsewhere".to_owned());

        let items = session_catalog_items(vec![mine, other], Some("workspace-current".to_owned()));

        assert_eq!(
            items
                .iter()
                .map(|item| item.session_id.as_str())
                .collect::<Vec<_>>(),
            ["s-mine"],
            "别的工作区的会话不许混进当前工作区的列表"
        );
    }

    #[test]
    fn session_catalog_items_without_a_resolved_workspace_keep_every_row() {
        let items = session_catalog_items(
            vec![session_row("s-a", "openai", "gpt-5.5", None, false)],
            None,
        );

        assert_eq!(items.len(), 1, "工作区解析不出来时不按工作区收敛");
    }
}
