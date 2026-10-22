use std::{collections::BTreeMap, path::Path};

use axon_agent_runtime::McpHttpAuthConfig;
use axon_agent_runtime::{
    AgentToolRegistryRuntime, McpPersistentRuntime, McpServerConfig, McpStartupProjection,
    McpToolRegistryRuntime, McpToolSchema, McpTransportRuntime, ProviderToolCall, ToolRegistry,
    mcp_provider_tool_name,
};
use axon_domain::{McpServerStartupStatus, ToolId};
use axon_time::unix_now_ms;

use super::{
    discovery::{McpServerSource, RuntimeMcpServer, discover_runtime_servers, runtime_home},
    mcp_call_support::{RuntimeMcpToolCallError, mcp_call_timeout},
    mcp_health_cache::{
        cached_mcp_health_failure, cached_mcp_tool_cooldown, clear_mcp_health_cache,
        clear_mcp_health_failure, clear_mcp_tool_cooldown, record_mcp_health_failure,
        record_mcp_tool_cooldown,
    },
    mcp_schema_cache::McpSchemaCacheDisposition,
    mcp_tool_output::{
        RuntimeMcpToolCallOutput, runtime_mcp_http_output, runtime_mcp_stdio_output,
    },
};
mod mcp_schema_support;
use mcp_schema_support::{load_runtime_mcp_tool_schemas, mcp_failure_text};

#[cfg(test)]
#[path = "mcp_contract_tests.rs"]
mod mcp_contract_tests;

const RUNTIME_MCP_TOOL_FIRST_ID: u64 = 10_000;

#[derive(Debug, Clone)]
struct RuntimeMcpToolContext {
    registry: ToolRegistry,
    routes: BTreeMap<String, RuntimeMcpToolRoute>,
    cache_reports: Vec<RuntimeMcpServerLoadReport>,
}

/// 单个 MCP server 的加载结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMcpServerLoadReport {
    /// MCP server 稳定 ID。
    pub server_id: String,
    /// 最终生效定义的来源；重复定义已在发现阶段淘汰。
    pub source: McpServerSource,
    /// hit、stale、miss、empty 或 refreshed。
    pub disposition: &'static str,
    /// 已脱敏的加载失败原因。
    pub failure: Option<String>,
}

/// runtime 工具注册表及其 MCP schema 缓存证据。
#[derive(Debug, Clone)]
pub struct RuntimeToolRegistryBuild {
    /// 可直接用于本轮执行的工具注册表。
    pub registry: ToolRegistry,
    /// 每个 MCP server 的缓存状态。
    pub mcp_schema_cache: Vec<RuntimeMcpServerLoadReport>,
}

#[derive(Debug, Clone)]
struct RuntimeMcpToolRoute {
    definition: axon_domain::McpServerDefinition,
    server_id: String,
    source: McpServerSource,
    provider_name: String,
    registry_tool_name: String,
    config_ref: String,
    config: McpServerConfig,
    original_tool_name: String,
}

/// 构建包含内置工具与 runtime MCP tools 的工具注册表。
pub fn build_runtime_tool_registry(
    config_path: &Path,
    cwd: Option<&Path>,
) -> Result<ToolRegistry, String> {
    build_runtime_tool_registry_snapshot(config_path, cwd).map(|build| build.registry)
}

/// 构建工具注册表并返回 MCP schema 缓存证据。
pub fn build_runtime_tool_registry_snapshot(
    config_path: &Path,
    cwd: Option<&Path>,
) -> Result<RuntimeToolRegistryBuild, String> {
    build_runtime_tool_registry_snapshot_with_refresh(config_path, cwd, false)
}

/// 构建工具注册表，并按需绕过 MCP 健康负缓存执行恢复探测。
pub fn build_runtime_tool_registry_snapshot_with_refresh(
    config_path: &Path,
    cwd: Option<&Path>,
    refresh_mcp_health: bool,
) -> Result<RuntimeToolRegistryBuild, String> {
    let home = runtime_home();
    build_runtime_mcp_tool_context_with_home_refresh(
        config_path,
        cwd,
        home.as_deref(),
        refresh_mcp_health,
    )
    .map(|context| RuntimeToolRegistryBuild {
        registry: context.registry,
        mcp_schema_cache: context.cache_reports,
    })
}

/// 使用显式 home 构建 runtime 工具注册表，供跨 crate 测试固定发现范围。
pub fn build_runtime_tool_registry_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Result<ToolRegistry, String> {
    build_runtime_tool_registry_with_home(config_path, cwd, home)
}

/// 使用显式 home 构建 runtime 工具注册表及 MCP schema 缓存证据，供跨 crate 测试固定发现范围。
pub fn build_runtime_tool_registry_snapshot_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Result<RuntimeToolRegistryBuild, String> {
    build_runtime_mcp_tool_context_with_home_refresh(config_path, cwd, home, false).map(|context| {
        RuntimeToolRegistryBuild {
            registry: context.registry,
            mcp_schema_cache: context.cache_reports,
        }
    })
}

fn build_runtime_tool_registry_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
) -> Result<ToolRegistry, String> {
    build_runtime_mcp_tool_context_with_home_refresh(config_path, cwd, home, false)
        .map(|context| context.registry)
}

fn build_runtime_tool_registry_with_home_refresh(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    refresh_mcp_health: bool,
) -> Result<ToolRegistry, String> {
    build_runtime_mcp_tool_context_with_home_refresh(config_path, cwd, home, refresh_mcp_health)
        .map(|context| context.registry)
}

fn build_runtime_mcp_tool_context_with_home_refresh(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    refresh_mcp_health: bool,
) -> Result<RuntimeMcpToolContext, String> {
    let mut registry = ToolRegistry::new();
    let mut routes = BTreeMap::new();
    let mut cache_reports = Vec::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry)
        .map_err(|error| format!("builtin tool 注册失败：{error:?}"))?;
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| "无法解析用户级 Axon 目录".to_owned())?;
    let installs = discover_runtime_servers(&root, cwd, home)?;
    let mut next_tool_id = ToolId(RUNTIME_MCP_TOOL_FIRST_ID);

    for install in installs {
        let registration = register_runtime_mcp_install(
            &mut registry,
            &mut routes,
            &root,
            &install,
            refresh_mcp_health,
            next_tool_id,
        )?;
        if let Some(report) = registration.cache_report {
            cache_reports.push(report);
        }
        next_tool_id = ToolId(next_tool_id.0 + registration.registered_count as u64);
    }

    Ok(RuntimeMcpToolContext {
        registry,
        routes,
        cache_reports,
    })
}

/// 构建 runtime 工具注册表；失败时回退到内置工具，并可刷新 MCP 健康负缓存。
#[must_use]
pub fn runtime_tool_registry_or_builtin_with_refresh(
    config_path: Option<&Path>,
    cwd: Option<&Path>,
    refresh_mcp_health: bool,
) -> ToolRegistry {
    match config_path {
        Some(path) => {
            let home = runtime_home();
            build_runtime_tool_registry_with_home_refresh(
                path,
                cwd,
                home.as_deref(),
                refresh_mcp_health,
            )
            .unwrap_or_else(|_| builtin_tool_registry())
        }
        None => builtin_tool_registry(),
    }
}

/// 清理 MCP runtime 工具注册表健康负缓存，供管理面触发新一轮本机发现。
#[must_use]
pub fn clear_runtime_mcp_health_cache(root: &Path) -> usize {
    clear_mcp_health_cache(root)
}

/// 执行 provider tool call 对应的 MCP tool。
pub fn execute_runtime_mcp_tool_call(
    config_path: &Path,
    cwd: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
) -> Result<RuntimeMcpToolCallOutput, RuntimeMcpToolCallError> {
    execute_runtime_mcp_tool_call_with_cancel(config_path, cwd, execution_registry, tool_call, None)
}

/// 执行 provider tool call，并在运行取消时中断本机 stdio MCP 子进程。
pub fn execute_runtime_mcp_tool_call_with_cancel(
    config_path: &Path,
    cwd: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
    cancel_token: Option<&axon_agent_runtime::McpCancelToken>,
) -> Result<RuntimeMcpToolCallOutput, RuntimeMcpToolCallError> {
    let home = runtime_home();
    execute_runtime_mcp_tool_call_with_home(
        config_path,
        cwd,
        home.as_deref(),
        execution_registry,
        tool_call,
        cancel_token,
    )
}

/// 使用显式 home 执行 MCP tool call，供跨 crate 测试固定发现范围。
pub fn execute_runtime_mcp_tool_call_with_home_for_tests(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
) -> Result<RuntimeMcpToolCallOutput, RuntimeMcpToolCallError> {
    execute_runtime_mcp_tool_call_with_home(
        config_path,
        cwd,
        home,
        execution_registry,
        tool_call,
        None,
    )
}

fn execute_runtime_mcp_tool_call_with_home(
    config_path: &Path,
    cwd: Option<&Path>,
    home: Option<&Path>,
    execution_registry: &ToolRegistry,
    tool_call: &ProviderToolCall,
    cancel_token: Option<&axon_agent_runtime::McpCancelToken>,
) -> Result<RuntimeMcpToolCallOutput, RuntimeMcpToolCallError> {
    let context = build_runtime_mcp_tool_context_with_home_refresh(config_path, cwd, home, false)
        .map_err(RuntimeMcpToolCallError::unavailable)?;
    let provider_name = tool_call.dispatch_name();
    let Some(route) = context.routes.get(provider_name).cloned() else {
        return Err(RuntimeMcpToolCallError::unavailable(format!(
            "未找到对应的 MCP tool：{provider_name}"
        )));
    };
    if route.provider_name != provider_name {
        return Err(RuntimeMcpToolCallError::unavailable(
            "MCP tool route 名称不一致",
        ));
    }
    ensure_runtime_mcp_contract_unchanged(execution_registry, &context.registry, provider_name)?;
    let root = axon_config::runtime_axon_root_path(config_path)
        .map_err(|_| RuntimeMcpToolCallError::invalid_config("无法解析用户级 Axon 目录"))?;
    // 调用期只设快速失败闸门，绝不写 server 级故障：server 级熔断只由发现期故障驱动，
    // 否则一次工具调用抖动就会摘掉整台 server 的工具。
    if let Some(failure) = cached_mcp_health_failure(&root, &route.config_ref, false) {
        return Err(RuntimeMcpToolCallError::server_circuit_open(format!(
            "MCP server {} 处于短期熔断：{failure}",
            route.server_id
        )));
    }
    if let Some(reason) = cached_mcp_tool_cooldown(&root, &route.config_ref, &route.provider_name) {
        return Err(RuntimeMcpToolCallError::tool_cooling(format!(
            "MCP tool {} 处于短期冷却：{reason}",
            route.provider_name
        )));
    }

    let startup = McpStartupProjection {
        server_id: route.server_id.clone(),
        config_ref: route.config_ref.clone(),
        status: McpServerStartupStatus::Discovered,
        server_process_started: false,
        remote_trigger_allowed: false,
        audit_summary: format!(
            "MCP server {} tool 已进入 turn-time 执行；source={}",
            route.server_id,
            route.source.as_str()
        ),
        reasons: Vec::new(),
    };
    let config = route.config.clone();
    let timeout = mcp_call_timeout(&config).map_err(RuntimeMcpToolCallError::invalid_config)?;
    let mcp_tool_name = route.original_tool_name.clone();
    execution_registry
        .validate_call_input(provider_name, &tool_call.input)
        .map_err(|error| {
            RuntimeMcpToolCallError::invalid_input(format!("MCP tool 入参校验失败：{error}"))
        })?;
    let arguments = tool_call
        .input
        .as_json()
        .ok_or_else(|| RuntimeMcpToolCallError::invalid_input("MCP 工具不接受 freeform 输入"))?;
    let arguments_json = serde_json::to_string(arguments)
        .map_err(|_| RuntimeMcpToolCallError::invalid_input("MCP arguments JSON 序列化失败"))?;
    let output = match config {
        McpServerConfig::Stdio(command) => execute_runtime_mcp_stdio_call(
            &root,
            &route,
            &startup,
            &command,
            &mcp_tool_name,
            &arguments_json,
            RuntimeMcpCallControl {
                timeout,
                cancel_token,
            },
        )?,
        McpServerConfig::StreamableHttp(http) => {
            let auth_config = McpHttpAuthConfig::from_http_server(&http);
            let report = McpTransportRuntime::call_streamable_http_tool(
                http.url,
                &http.headers,
                &auth_config,
                &mcp_tool_name,
                &arguments_json,
                timeout,
            );
            let report = match report {
                Ok(report) => report,
                Err(error) => {
                    let message = format!("MCP HTTP tool 调用失败：{error:?}");
                    if message.contains("MCP tools/call 返回 error") {
                        return Err(RuntimeMcpToolCallError::tool_rejected(message));
                    }
                    return Err(record_mcp_call_failure(&root, &route, &message));
                }
            };
            runtime_mcp_http_output(
                &route.server_id,
                &route.registry_tool_name,
                &mcp_tool_name,
                &report,
            )
        }
    };
    clear_mcp_health_failure(&root, &startup.config_ref);
    clear_mcp_tool_cooldown(&root, &route.config_ref, &route.provider_name);
    Ok(output)
}

/// 记录调用期失败：只冷却 (server, tool)，绝不写 server 级故障。
///
/// 调用期异常是单次调用的事实，不是整台 server 的健康结论；写 server 级熔断会让同 server
/// 的其他健康工具一起从模型可见工具表中消失，并让它们接下来的调用全部被快速拒绝。
fn record_mcp_call_failure(
    root: &Path,
    route: &RuntimeMcpToolRoute,
    message: &str,
) -> RuntimeMcpToolCallError {
    record_mcp_tool_cooldown(root, &route.config_ref, &route.provider_name, message);
    RuntimeMcpToolCallError::unavailable(message)
}

fn ensure_runtime_mcp_contract_unchanged(
    execution_registry: &ToolRegistry,
    discovered_registry: &ToolRegistry,
    provider_name: &str,
) -> Result<(), RuntimeMcpToolCallError> {
    let expected = execution_registry
        .tool_by_qualified_name(provider_name)
        .ok_or_else(|| {
            RuntimeMcpToolCallError::invalid_input(format!(
                "当轮工具快照未暴露 MCP tool：{provider_name}"
            ))
        })?;
    let discovered = discovered_registry
        .tool_by_qualified_name(provider_name)
        .ok_or_else(|| {
            RuntimeMcpToolCallError::unavailable(format!(
                "MCP tool 在执行前已不可用：{provider_name}"
            ))
        })?;
    if expected.schema != discovered.schema
        || expected.permission_policy != discovered.permission_policy
        || expected.concurrency_policy != discovered.concurrency_policy
    {
        return Err(RuntimeMcpToolCallError::unavailable(format!(
            "MCP tool 契约在执行前发生漂移，已拒绝本轮调用：{provider_name}"
        )));
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct RuntimeMcpCallControl<'a> {
    timeout: Option<std::time::Duration>,
    cancel_token: Option<&'a axon_agent_runtime::McpCancelToken>,
}

fn execute_runtime_mcp_stdio_call(
    root: &Path,
    route: &RuntimeMcpToolRoute,
    startup: &McpStartupProjection,
    command: &axon_agent_runtime::McpServerCommand,
    tool_name: &str,
    arguments_json: &str,
    control: RuntimeMcpCallControl<'_>,
) -> Result<RuntimeMcpToolCallOutput, RuntimeMcpToolCallError> {
    let mut session =
        McpPersistentRuntime::connect(&route.definition, startup, command).map_err(|error| {
            let failure = mcp_failure_text(&error);
            axon_logger::error(&format!(
                "MCP stdio 启动失败：server_id={} config_ref={} phase=persistent_connect failure={}",
                route.server_id, startup.config_ref, failure
            ));
            record_mcp_call_failure(root, route, &format!("MCP server 启动失败：{failure}"))
        })?;
    let report = match session.call_tool(
        tool_name,
        arguments_json,
        control.timeout,
        control.cancel_token,
    ) {
        Ok(report) => report,
        Err(error) => {
            let failure = mcp_failure_text(&error);
            let message = format!("MCP tool 调用失败：{failure}");
            if message.contains("MCP tools/call 返回 error") {
                return Err(RuntimeMcpToolCallError::tool_rejected(message));
            }
            if control
                .cancel_token
                .is_some_and(|is_cancelled| is_cancelled())
            {
                // 用户中断是运行期控制事实，不是 server 健康信号：既不熔断也不冷却。
                axon_logger::warning(&format!(
                    "MCP stdio 调用已取消：server_id={} config_ref={} phase=tools_call",
                    route.server_id, startup.config_ref
                ));
                return Err(RuntimeMcpToolCallError::unavailable(message));
            }
            axon_logger::error(&format!(
                "MCP stdio 调用失败：server_id={} config_ref={} phase=tools_call failure={}",
                route.server_id, startup.config_ref, failure
            ));
            return Err(record_mcp_call_failure(root, route, &message));
        }
    };
    session.shutdown();
    Ok(runtime_mcp_stdio_output(
        &route.server_id,
        &route.registry_tool_name,
        tool_name,
        &report,
    ))
}

/// 构建 runtime 工具注册表；失败或未提供配置路径时回退到内置工具。
#[must_use]
pub fn runtime_tool_registry_or_builtin(config_path: Option<&Path>) -> ToolRegistry {
    runtime_tool_registry_or_builtin_with_cwd(config_path, None)
}

/// 构建 runtime 工具注册表，并用 cwd 发现项目级插件来源。
#[must_use]
pub fn runtime_tool_registry_or_builtin_with_cwd(
    config_path: Option<&Path>,
    cwd: Option<&Path>,
) -> ToolRegistry {
    runtime_tool_registry_or_builtin_with_refresh(config_path, cwd, false)
}

/// 使用显式 home 构建 runtime 工具注册表；失败时回退到内置工具，供跨 crate 测试隔离本机环境。
#[must_use]
pub fn runtime_tool_registry_or_builtin_with_home_for_tests(
    config_path: Option<&Path>,
    cwd: Option<&Path>,
    home: Option<&Path>,
    refresh_mcp_health: bool,
) -> ToolRegistry {
    match config_path {
        Some(path) => {
            build_runtime_tool_registry_with_home_refresh(path, cwd, home, refresh_mcp_health)
                .unwrap_or_else(|_| builtin_tool_registry())
        }
        None => builtin_tool_registry(),
    }
}

fn builtin_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    if AgentToolRegistryRuntime::register_builtin_tools(&mut registry).is_err() {
        return ToolRegistry::new();
    }
    registry
}

struct RuntimeMcpServerRouteContext<'a> {
    definition: &'a axon_domain::McpServerDefinition,
    server_id: &'a str,
    source: McpServerSource,
    config_ref: &'a str,
    config: &'a McpServerConfig,
}

fn register_mcp_tool_routes(
    routes: &mut BTreeMap<String, RuntimeMcpToolRoute>,
    context: &RuntimeMcpServerRouteContext<'_>,
    registered_tools: &[axon_domain::ToolDefinition],
    registered_schemas: &[McpToolSchema],
) {
    for (tool, schema) in registered_tools.iter().zip(registered_schemas) {
        let provider_name = mcp_provider_tool_name(context.server_id, &tool.name);
        routes.insert(
            provider_name.clone(),
            RuntimeMcpToolRoute {
                definition: context.definition.clone(),
                server_id: context.server_id.to_owned(),
                source: context.source,
                provider_name,
                registry_tool_name: tool.name.clone(),
                config_ref: context.config_ref.to_owned(),
                config: context.config.clone(),
                original_tool_name: schema.mcp_name.clone(),
            },
        );
    }
}

fn runtime_now_ms() -> u64 {
    unix_now_ms()
}

struct RuntimeMcpInstallIdentity {
    server_id: String,
    source: McpServerSource,
    startup: McpStartupProjection,
}

struct RuntimeMcpInstallRegistration {
    registered_count: usize,
    cache_report: Option<RuntimeMcpServerLoadReport>,
}

struct RuntimeMcpSchemaLoad {
    loaded: Option<(McpServerConfig, Vec<McpToolSchema>)>,
    disposition: McpSchemaCacheDisposition,
    failure: Option<String>,
}

fn register_runtime_mcp_install(
    registry: &mut ToolRegistry,
    routes: &mut BTreeMap<String, RuntimeMcpToolRoute>,
    root: &Path,
    install: &RuntimeMcpServer,
    refresh_mcp_health: bool,
    next_tool_id: ToolId,
) -> Result<RuntimeMcpInstallRegistration, String> {
    let Some(identity) = runtime_mcp_install_identity(install)? else {
        return Ok(RuntimeMcpInstallRegistration {
            registered_count: 0,
            cache_report: None,
        });
    };
    let load = load_runtime_mcp_tool_schemas(
        root,
        registry,
        install,
        &identity.startup,
        &identity.server_id,
        refresh_mcp_health,
    );
    let mut cache_report = Some(RuntimeMcpServerLoadReport {
        server_id: identity.server_id.clone(),
        source: identity.source,
        disposition: load.disposition.as_str(),
        failure: load.failure,
    });
    let Some((config, tool_schemas)) = load.loaded else {
        return Ok(RuntimeMcpInstallRegistration {
            registered_count: 0,
            cache_report,
        });
    };
    let report = match McpToolRegistryRuntime::register_tool_schemas(
        registry,
        &install.definition,
        &identity.startup,
        &identity.server_id,
        &tool_schemas,
        next_tool_id,
    ) {
        Ok(report) => report,
        Err(error) => {
            let failure = format!("MCP schema 注册失败：{error:?}");
            record_mcp_health_failure(root, &identity.startup.config_ref, &failure);
            if let Some(report) = &mut cache_report {
                report.failure = Some(failure);
            }
            return Ok(RuntimeMcpInstallRegistration {
                registered_count: 0,
                cache_report,
            });
        }
    };
    clear_mcp_health_failure(root, &identity.startup.config_ref);
    let route_context = RuntimeMcpServerRouteContext {
        definition: &install.definition,
        server_id: &identity.server_id,
        source: identity.source,
        config_ref: &identity.startup.config_ref,
        config: &config,
    };
    register_mcp_tool_routes(
        routes,
        &route_context,
        &report.registered_tools,
        &report.registered_schemas,
    );
    Ok(RuntimeMcpInstallRegistration {
        registered_count: report.registered_tools.len(),
        cache_report,
    })
}

fn runtime_mcp_install_identity(
    install: &RuntimeMcpServer,
) -> Result<Option<RuntimeMcpInstallIdentity>, String> {
    let server_id = install.definition.server_id.clone();
    let source = install.source;
    let startup = McpStartupProjection {
        server_id: server_id.clone(),
        config_ref: install.config_ref(),
        status: McpServerStartupStatus::Discovered,
        server_process_started: false,
        remote_trigger_allowed: false,
        audit_summary: format!("MCP server {server_id} tools/list 已进入 turn-time 装配"),
        reasons: Vec::new(),
    };
    Ok(Some(RuntimeMcpInstallIdentity {
        server_id,
        source,
        startup,
    }))
}
