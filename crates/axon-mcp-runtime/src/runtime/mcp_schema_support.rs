use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    thread,
};

use axon_agent_runtime::{
    McpHttpAuthConfig, McpServerConfig, McpServerMetadataRequest, McpServerMetadataRuntime,
    McpStartupProjection, McpStdioSessionRuntime, McpToolSchema, McpTransportRuntime, ToolRegistry,
};

use super::super::discovery::RuntimeMcpServer;
use super::super::mcp_call_support::normalized_description;
use super::super::mcp_health_cache::{
    cached_mcp_health_failure, clear_mcp_health_failure, record_mcp_health_failure,
};
use super::super::mcp_runtime_oauth::config_with_runtime_oauth;
use super::super::mcp_schema_cache::{
    McpSchemaCacheDisposition, McpSchemaCacheEntry, McpSchemaCacheLookup,
    cached_metadata_fingerprint, lookup_mcp_schemas, lookup_mcp_schemas_ignoring_ttl,
    mcp_config_fingerprint, mcp_initialize_fingerprint, store_mcp_schemas,
};
use super::{RuntimeMcpSchemaLoad, builtin_tool_registry, runtime_now_ms};

/// 沿用缓存目录时写进工具描述的可见标记。
///
/// 发现期只熔断 30 秒，但模型看到的却是「这台 server 的工具凭空消失」，并会据此判定该能力
/// 不可用。标记只带 server 标识、不带失败原文：同一状态下的两次构建必须生成同一段描述，
/// 否则调用前的契约比对会把一次正常调用误判成 schema 漂移。
const MCP_DEGRADED_TOOL_MARKER: &str = "[暂时不可用] 该 MCP server 的发现期探测失败，此处沿用上次成功的工具目录；本轮调用会被快速拒绝，稍后重试即可。";

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
    let fingerprint = mcp_config_fingerprint(&config);
    if !refresh_mcp_health
        && let Some(failure) = cached_mcp_health_failure(root, &startup.config_ref, false)
    {
        // 熔断窗口内不拉起 server，但也不让工具整批消失：有上次成功目录就照旧注册，
        // 并把「暂时不可用」写进描述。工具调用期会命中同一条负缓存并返回可读的熔断原因。
        if let Some(schemas) =
            lookup_mcp_schemas_ignoring_ttl(root, server_id, &startup.config_ref, &fingerprint)
        {
            axon_logger::warning(&format!(
                "MCP tools/list 使用健康负缓存并沿用上次目录：server_id={} config_ref={} phase=health_cache cached=true failure={}",
                server_id, startup.config_ref, failure
            ));
            return RuntimeMcpSchemaLoad {
                loaded: Some((config, mark_degraded_schemas(server_id, schemas))),
                disposition: McpSchemaCacheDisposition::Degraded,
                failure: Some(failure),
            };
        }
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
    // initialize 结果与工具目录同一次落盘：server 自升级通常不动配置文件，只有版本号、
    // instructions 与 listChanged 会变，把它们记进缓存行才能让「目录是哪个 server 版本产的」可查。
    let (metadata_fingerprint, tools_list_changed) = read_runtime_mcp_metadata(&config, server_id);
    report_metadata_drift(root, server_id, &metadata_fingerprint);
    let _ = store_mcp_schemas(
        root,
        &McpSchemaCacheEntry {
            server_id,
            config_ref: &startup.config_ref,
            config_fingerprint: &fingerprint,
            metadata_fingerprint: &metadata_fingerprint,
            tools_list_changed,
            refreshed_at_ms: runtime_now_ms(),
        },
        &tool_schemas,
    );
    RuntimeMcpSchemaLoad {
        loaded: Some((config, tool_schemas)),
        disposition: McpSchemaCacheDisposition::Refreshed,
        failure: None,
    }
}

/// 读一次 initialize 结果，取 server 版本、instructions 与 `tools.listChanged`。
///
/// 只有真正刷新目录时才调用：命中缓存不握手，因此这里多一次握手的代价只落在过期刷新
/// 与首次装配上。读不到就返回空指纹（等价于「版本未知」），绝不因为元数据探测失败而
/// 丢掉刚拿到的工具目录。
fn read_runtime_mcp_metadata(config: &McpServerConfig, server_id: &str) -> (String, Option<bool>) {
    match McpServerMetadataRuntime::read_metadata(McpServerMetadataRequest {
        config: config.clone(),
    }) {
        Ok(metadata) => {
            if matches!(metadata.capabilities.tools_list_changed, Some(true)) {
                axon_logger::info(&format!(
                    "MCP server 声明 tools.listChanged：目录缓存改用短 TTL。server_id={server_id} version={:?}",
                    metadata.version
                ));
            }
            let fingerprint = mcp_initialize_fingerprint(&metadata);
            (fingerprint, metadata.capabilities.tools_list_changed)
        }
        Err(error) => {
            axon_logger::warning(&format!(
                "MCP initialize 元数据探测失败，目录缓存按未知版本记录：server_id={server_id} error={}",
                mcp_failure_text(&error)
            ));
            (String::new(), None)
        }
    }
}

fn mark_degraded_schemas(server_id: &str, mut schemas: Vec<McpToolSchema>) -> Vec<McpToolSchema> {
    let marker = format!("\n\n{MCP_DEGRADED_TOOL_MARKER}（server={server_id}）");
    for schema in &mut schemas {
        if !schema.description.contains(MCP_DEGRADED_TOOL_MARKER) {
            schema.description.push_str(&marker);
        }
    }
    schemas
}

/// 对比新旧 initialize 指纹，把「server 升级导致目录重建」留成可查事实。
///
/// 空指纹表示这次元数据探测没成功，既不能当成「没变」，也不能当成「变了」。
fn report_metadata_drift(root: &Path, server_id: &str, metadata_fingerprint: &str) {
    if metadata_fingerprint.is_empty() {
        return;
    }
    let Some((previous, refreshed_at_ms)) = cached_metadata_fingerprint(root, server_id) else {
        return;
    };
    if previous.is_empty() || previous == metadata_fingerprint {
        return;
    }
    axon_logger::info(&format!(
        "MCP server initialize 指纹变化，工具目录已重建：server_id={server_id} cached_age_ms={}",
        runtime_now_ms().saturating_sub(refreshed_at_ms)
    ));
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
                    } else {
                        let (metadata_fingerprint, tools_list_changed) =
                            read_runtime_mcp_metadata(&config, &server_id);
                        if store_mcp_schemas(
                            &root,
                            &McpSchemaCacheEntry {
                                server_id: &server_id,
                                config_ref: &startup.config_ref,
                                config_fingerprint: &fingerprint,
                                metadata_fingerprint: &metadata_fingerprint,
                                tools_list_changed,
                                refreshed_at_ms: runtime_now_ms(),
                            },
                            &schemas,
                        )
                        .is_ok()
                        {
                            clear_mcp_health_failure(&root, &startup.config_ref);
                        }
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

#[cfg(test)]
#[path = "mcp_schema_support_tests.rs"]
mod tests;
