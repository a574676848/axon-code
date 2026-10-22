use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
};

use axon_agent_runtime::{
    McpHttpAuthConfig, McpServerConfig, McpStartupProjection, McpStdioSessionRuntime,
    McpToolSchema, McpTransportRuntime, ToolRegistry,
};

use super::super::discovery::RuntimeMcpServer;
use super::super::mcp_call_support::normalized_description;
use super::super::mcp_health_cache::{
    cached_mcp_health_failure, clear_mcp_health_failure, record_mcp_health_failure,
};
use super::super::mcp_runtime_oauth::config_with_runtime_oauth;
use super::super::mcp_schema_cache::{
    McpSchemaCacheDisposition, McpSchemaCacheLookup, lookup_mcp_schemas, mcp_config_fingerprint,
    store_mcp_schemas,
};
use super::{RuntimeMcpSchemaLoad, builtin_tool_registry, runtime_now_ms};

pub(super) fn load_runtime_mcp_tool_schemas(
    root: &Path,
    registry: &ToolRegistry,
    install: &RuntimeMcpServer,
    startup: &McpStartupProjection,
    server_id: &str,
    refresh_mcp_health: bool,
) -> RuntimeMcpSchemaLoad {
    // OAuth 登录保存的 access token 必须在 tools/list 装配阶段注入：注册后的 route 会复用
    // 同一份配置执行 tools/call，漏注入会让授权成功的 server 持续返回 401。
    let config = match install.mcp_config() {
        Ok(config) => config_with_runtime_oauth(root, server_id, config),
        Err(error) => {
            let failure = format!("MCP 配置加载失败：{error}");
            axon_logger::error(&format!(
                "MCP 配置加载失败：server_id={} config_ref={} phase=config_load failure={}",
                server_id, startup.config_ref, failure
            ));
            record_mcp_health_failure(root, &startup.config_ref, &failure);
            return RuntimeMcpSchemaLoad {
                loaded: None,
                disposition: McpSchemaCacheDisposition::Miss,
                failure: Some(failure),
            };
        }
    };
    if !refresh_mcp_health
        && let Some(failure) = cached_mcp_health_failure(root, &startup.config_ref, false)
    {
        axon_logger::warning(&format!(
            "MCP tools/list 使用健康负缓存：server_id={} config_ref={} phase=health_cache cached=true failure={}",
            server_id, startup.config_ref, failure
        ));
        return RuntimeMcpSchemaLoad {
            loaded: None,
            disposition: McpSchemaCacheDisposition::Miss,
            failure: Some(failure),
        };
    }
    let fingerprint = mcp_config_fingerprint(&config);
    if !refresh_mcp_health {
        match lookup_mcp_schemas(
            root,
            server_id,
            &startup.config_ref,
            &fingerprint,
            runtime_now_ms(),
        ) {
            McpSchemaCacheLookup::Hit(schemas) => {
                return RuntimeMcpSchemaLoad {
                    loaded: Some((config, schemas)),
                    disposition: McpSchemaCacheDisposition::Hit,
                    failure: None,
                };
            }
            McpSchemaCacheLookup::Stale(schemas) => {
                spawn_stale_schema_refresh(
                    root.to_path_buf(),
                    install.clone(),
                    startup.clone(),
                    server_id.to_owned(),
                    config.clone(),
                    fingerprint,
                );
                return RuntimeMcpSchemaLoad {
                    loaded: Some((config, schemas)),
                    disposition: McpSchemaCacheDisposition::Stale,
                    failure: None,
                };
            }
            McpSchemaCacheLookup::Empty | McpSchemaCacheLookup::Miss => {}
        }
    }
    let tool_schemas = match list_live_runtime_mcp_tool_schemas(registry, install, startup, &config)
    {
        Ok(tool_schemas) => tool_schemas,
        Err(failure) => {
            axon_logger::error(&format!(
                "MCP tools/list 失败：server_id={} config_ref={} phase=tools_list failure={}",
                server_id, startup.config_ref, failure
            ));
            record_mcp_health_failure(root, &startup.config_ref, &failure);
            return RuntimeMcpSchemaLoad {
                loaded: None,
                disposition: McpSchemaCacheDisposition::Miss,
                failure: Some(failure),
            };
        }
    };
    let tool_schemas = normalize_runtime_mcp_schemas(&tool_schemas);
    if tool_schemas.is_empty() {
        let failure = "MCP tools/list 返回空工具列表".to_owned();
        axon_logger::warning(&format!(
            "MCP tools/list 返回空工具列表：server_id={} config_ref={} phase=tools_list",
            server_id, startup.config_ref
        ));
        record_mcp_health_failure(root, &startup.config_ref, &failure);
        return RuntimeMcpSchemaLoad {
            loaded: None,
            disposition: McpSchemaCacheDisposition::Empty,
            failure: Some(failure),
        };
    }
    let _ = store_mcp_schemas(
        root,
        server_id,
        &startup.config_ref,
        &fingerprint,
        runtime_now_ms(),
        &tool_schemas,
    );
    RuntimeMcpSchemaLoad {
        loaded: Some((config, tool_schemas)),
        disposition: McpSchemaCacheDisposition::Refreshed,
        failure: None,
    }
}

fn normalize_runtime_mcp_schemas(schemas: &[McpToolSchema]) -> Vec<McpToolSchema> {
    schemas.iter().map(normalize_runtime_mcp_schema).collect()
}

fn normalize_runtime_mcp_schema(schema: &McpToolSchema) -> McpToolSchema {
    McpToolSchema {
        name: schema.name.clone(),
        mcp_name: if schema.mcp_name.trim().is_empty() {
            schema.name.clone()
        } else {
            schema.mcp_name.clone()
        },
        description: normalized_description(&schema.description),
        input_schema: schema.input_schema.clone(),
        output_schema: schema.output_schema.clone(),
        annotations: schema.annotations,
    }
}

pub(super) fn list_live_runtime_mcp_tool_schemas(
    registry: &ToolRegistry,
    install: &RuntimeMcpServer,
    startup: &McpStartupProjection,
    config: &McpServerConfig,
) -> Result<Vec<McpToolSchema>, String> {
    match config {
        McpServerConfig::Stdio(command) => McpStdioSessionRuntime::discover_and_list_tools(
            &install.definition,
            startup,
            command,
            registry,
        )
        .map(|session| session.tool_schemas)
        .map_err(|error| format!("MCP stdio tools/list 失败：{}", mcp_failure_text(&error))),
        McpServerConfig::StreamableHttp(http) => {
            let auth_config = McpHttpAuthConfig::from_http_server(http);
            McpTransportRuntime::list_streamable_http_tools_with_pagination(
                http.url.clone(),
                &http.headers,
                &auth_config,
            )
            .map(|report| report.tool_schemas)
            .map_err(|error| format!("MCP HTTP tools/list 失败：{}", mcp_failure_text(&error)))
        }
    }
}

/// 把 MCP 传输错误投影为可读失败文本。
///
/// `InvalidMcpDefinition` 已经携带面向用户的中文原因（例如 OAuth 授权引导），
/// 直接采用该原因；运行时失败采用动态消息，其余错误保留 Debug 结构，便于定位协议或 IO 细节。
pub(super) fn mcp_failure_text(error: &axon_domain::AxonError) -> String {
    match error {
        axon_domain::AxonError::InvalidMcpDefinition(reason) => (*reason).to_owned(),
        axon_domain::AxonError::Runtime(axon_domain::RuntimeError::Failure(reason)) => {
            reason.clone()
        }
        other => format!("{other:?}"),
    }
}

fn spawn_stale_schema_refresh(
    root: PathBuf,
    install: RuntimeMcpServer,
    startup: McpStartupProjection,
    server_id: String,
    config: McpServerConfig,
    fingerprint: String,
) {
    let refresh_key = format!("{server_id}:{fingerprint}");
    {
        let mut active = schema_refreshes()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !active.insert(refresh_key.clone()) {
            return;
        }
    }
    let _ = thread::Builder::new()
        .name(format!("axon-mcp-schema-refresh-{server_id}"))
        .spawn(move || {
            let registry = builtin_tool_registry();
            match list_live_runtime_mcp_tool_schemas(&registry, &install, &startup, &config) {
                Ok(schemas) => {
                    let schemas = normalize_runtime_mcp_schemas(&schemas);
                    if schemas.is_empty() {
                        record_mcp_health_failure(
                            &root,
                            &startup.config_ref,
                            "MCP tools/list 返回空工具列表",
                        );
                    } else if store_mcp_schemas(
                        &root,
                        &server_id,
                        &startup.config_ref,
                        &fingerprint,
                        runtime_now_ms(),
                        &schemas,
                    )
                    .is_ok()
                    {
                        clear_mcp_health_failure(&root, &startup.config_ref);
                    }
                }
                Err(failure) => {
                    record_mcp_health_failure(&root, &startup.config_ref, &failure);
                }
            }
            schema_refreshes()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&refresh_key);
        });
}

fn schema_refreshes() -> &'static Mutex<BTreeSet<String>> {
    static REFRESHES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
    REFRESHES.get_or_init(|| Mutex::new(BTreeSet::new()))
}
