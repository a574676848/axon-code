//! 插件 MCP server 进程与工具调用转发 runtime。

use std::collections::BTreeMap;
use std::io::Write;

use axon_domain::{AxonError, AxonResult, McpServerDefinition, ToolInput, ToolOutput, ToolResult};
use serde_json::json;

use crate::{
    McpStartupProjection, ToolRegistry, is_mcp_tool_namespace,
    mcp_stdio_launcher::{McpStdioChild, spawn_mcp_stdio_child},
};

/// MCP tool call JSON-RPC 方法名。
pub const MCP_TOOL_CALL_METHOD: &str = "tools/call";
/// MCP server 启动成功的审计原因。
pub const MCP_PROCESS_REASON_STARTED: &str = "mcp_process:started";
/// MCP server 健康检查为 running 的审计原因。
pub const MCP_HEALTH_REASON_RUNNING: &str = "mcp_health:running";
/// MCP server 健康检查为 exited 的审计原因。
pub const MCP_HEALTH_REASON_EXITED: &str = "mcp_health:exited";
const MCP_JSONRPC_VERSION: &str = "2.0";
const MCP_TOOL_CALL_ID: u64 = 1;

/// 插件 MCP server 启动命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerCommand {
    /// 可执行程序。
    pub program: String,
    /// 命令参数。
    pub args: Vec<String>,
    /// 工作目录。
    pub working_dir: Option<String>,
    /// 子进程环境变量。
    pub env: BTreeMap<String, String>,
    /// 单次 tools/call 超时，单位毫秒；未配置时使用 MCP 运行预算默认值。
    pub tool_timeout_ms: Option<u64>,
}

/// 插件 MCP HTTP header 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerHeader {
    /// Header 名称。
    pub name: String,
    /// Header 值。
    pub value: String,
}

/// 插件 MCP HTTP transport 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpHttpServerConfig {
    /// MCP endpoint URL。
    pub url: String,
    /// 附加 HTTP header。
    pub headers: Vec<McpServerHeader>,
    /// Bearer token 环境变量名；token 值不会写入配置或 CLI 输出。
    pub bearer_token_env_var: Option<String>,
    /// 运行态 OAuth access token；仅由本地 credential store 注入，不允许写入配置文件。
    pub oauth_access_token: Option<String>,
    /// 单次 tools/call 超时，单位毫秒；未配置时使用 MCP 运行预算默认值。
    pub tool_timeout_ms: Option<u64>,
}

/// 插件 MCP server 配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerConfig {
    /// 本机 stdio server。
    Stdio(McpServerCommand),
    /// 当前 MCP 标准 Streamable HTTP transport。
    StreamableHttp(McpHttpServerConfig),
}

impl McpServerConfig {
    /// 返回 stdio command；非 stdio 配置返回 `None`。
    #[must_use]
    pub fn as_stdio_command(&self) -> Option<&McpServerCommand> {
        match self {
            Self::Stdio(command) => Some(command),
            Self::StreamableHttp(_) => None,
        }
    }
}

/// 插件 MCP server 健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerHealthStatus {
    /// 进程仍在运行。
    Running,
    /// 进程已退出。
    Exited,
}

/// 插件 MCP server 健康投影。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerHealth {
    /// 插件名称。
    pub server_id: String,
    /// 进程 ID。
    pub process_id: u32,
    /// 健康状态。
    pub status: McpServerHealthStatus,
    /// 退出码。
    pub exit_code: Option<i32>,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 插件 MCP server 进程句柄。
pub struct McpServerHandle {
    server_id: String,
    config_ref: String,
    process_id: u32,
    child: Option<McpStdioChild>,
}

/// 插件 MCP tool 调用请求。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolCallRequest {
    /// 工具 namespace。
    pub namespace: String,
    /// 模型可见工具名称，用于本地注册表校验和结果物化。
    pub name: String,
    /// MCP server 原始工具名称，用于 `tools/call` 协议请求。
    pub mcp_name: String,
    /// 工具入参字段。
    pub input: ToolInput,
    /// JSON 参数文本。
    pub arguments_json: String,
}

/// 插件 MCP tool 调用报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolCallReport {
    /// 规范化后的工具结果。
    pub result: ToolResult,
    /// MCP server 原始 result 文本。
    pub result_text: String,
    /// 调用后的健康状态。
    pub health: McpServerHealth,
    /// 转发给 MCP server 的 JSON-RPC 请求。
    pub forwarded_request_json: String,
    /// 是否允许远控触发。
    pub remote_trigger_allowed: bool,
}

/// 插件 MCP 进程 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpProcessRuntime;

impl McpProcessRuntime {
    /// 启动插件 MCP server 子进程。
    pub fn start_server(
        report: &McpServerDefinition,
        startup: &McpStartupProjection,
        command: &McpServerCommand,
    ) -> AxonResult<McpServerHandle> {
        ensure_can_start(report, startup, command)?;
        let child = spawn_mcp_stdio_child(command)?;
        let process_id = child.id();

        Ok(McpServerHandle {
            server_id: report.server_id.clone(),
            config_ref: startup.config_ref.clone(),
            process_id,
            child: Some(child),
        })
    }

    /// 投影 MCP server 健康状态。
    pub fn health(handle: &mut McpServerHandle) -> AxonResult<McpServerHealth> {
        let Some(child) = handle.child.as_mut() else {
            return Ok(mcp_health(
                handle,
                McpServerHealthStatus::Exited,
                None,
                MCP_HEALTH_REASON_EXITED,
            ));
        };
        let status = child
            .try_wait()
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP server 健康检查失败"))?;
        Ok(match status {
            Some(exit) => mcp_health(
                handle,
                McpServerHealthStatus::Exited,
                exit.code(),
                MCP_HEALTH_REASON_EXITED,
            ),
            None => mcp_health(
                handle,
                McpServerHealthStatus::Running,
                None,
                MCP_HEALTH_REASON_RUNNING,
            ),
        })
    }

    /// 将已注册的插件 MCP tool 调用转发给本机 MCP server 进程。
    pub fn call_tool(
        registry: &mut ToolRegistry,
        mut handle: McpServerHandle,
        request: &McpToolCallRequest,
    ) -> AxonResult<McpToolCallReport> {
        ensure_mcp_tool_request(registry, request)?;
        let request_json = tool_call_json(request)?;
        let Some(mut child) = handle.child.take() else {
            return Err(AxonError::InvalidMcpDefinition("MCP server 已结束"));
        };
        let Some(mut stdin) = child.stdin.take() else {
            return Err(AxonError::InvalidMcpDefinition("MCP server stdin 不可用"));
        };
        stdin
            .write_all(request_json.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP tool 调用写入失败"))?;
        drop(stdin);

        let output = child
            .wait_with_output()
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP tool 调用等待失败"))?;
        let health = mcp_health_from_output(&handle, output.status.code());
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let result = registry.materialize_output(
            &request.namespace,
            &request.name,
            &ToolOutput {
                text: stdout.clone(),
            },
            None,
        )?;

        Ok(McpToolCallReport {
            result,
            result_text: stdout,
            health,
            forwarded_request_json: request_json,
            remote_trigger_allowed: false,
        })
    }
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut()
            && child.try_wait().ok().flatten().is_none()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn ensure_can_start(
    report: &McpServerDefinition,
    startup: &McpStartupProjection,
    command: &McpServerCommand,
) -> AxonResult<()> {
    debug_assert!(!report.server_id.is_empty());
    if startup.server_process_started || startup.remote_trigger_allowed {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP server 只能由本机 runtime 启动",
        ));
    }
    if command.program.trim().is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP server 启动命令不能为空",
        ));
    }
    Ok(())
}

fn ensure_mcp_tool_request(
    registry: &ToolRegistry,
    request: &McpToolCallRequest,
) -> AxonResult<()> {
    if !is_mcp_tool_namespace(&request.namespace) {
        return Err(AxonError::tool_invalid_definition(
            "MCP tool 调用必须来自 MCP namespace",
        ));
    }
    registry.validate_input(&request.namespace, &request.name, &request.input)
}

fn tool_call_json(request: &McpToolCallRequest) -> AxonResult<String> {
    let arguments = if request.arguments_json.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&request.arguments_json)
            .map_err(|_| AxonError::tool_invalid_definition("MCP tool arguments 必须是 JSON"))?
    };
    serde_json::to_string(&json!({
        "jsonrpc": MCP_JSONRPC_VERSION,
        "id": MCP_TOOL_CALL_ID,
            "method": MCP_TOOL_CALL_METHOD,
            "params": {
            "name": request.mcp_name,
            "arguments": arguments,
        },
    }))
    .map_err(|_| AxonError::tool_invalid_definition("MCP tool 调用序列化失败"))
}

fn mcp_health(
    handle: &McpServerHandle,
    status: McpServerHealthStatus,
    exit_code: Option<i32>,
    reason: &str,
) -> McpServerHealth {
    McpServerHealth {
        server_id: handle.server_id.clone(),
        process_id: handle.process_id,
        status,
        exit_code,
        remote_trigger_allowed: false,
        reasons: vec![
            MCP_PROCESS_REASON_STARTED.to_owned(),
            reason.to_owned(),
            handle.config_ref.clone(),
        ],
    }
}

fn mcp_health_from_output(handle: &McpServerHandle, exit_code: Option<i32>) -> McpServerHealth {
    mcp_health(
        handle,
        McpServerHealthStatus::Exited,
        exit_code,
        MCP_HEALTH_REASON_EXITED,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use axon_domain::{McpServerStartupStatus, ToolId};

    use crate::{McpToolRegistryRuntime, McpToolSchema};

    #[test]
    fn mcp_process_forwards_registered_tool_call() {
        let report = mcp_report();
        let startup = startup_projection();
        let mut registry = ToolRegistry::new();
        McpToolRegistryRuntime::register_tool_schemas(
            &mut registry,
            &report,
            &startup,
            "team_tools",
            &[McpToolSchema {
                name: "query-db".to_owned(),
                mcp_name: "query-db".to_owned(),
                description: "查询数据库 schema".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": { "query": { "type": "string" } },
                    "required": ["query"]
                }),
                output_schema: None,
                annotations: crate::McpToolAnnotations::default(),
            }],
            ToolId(900),
        )
        .unwrap();
        let command = shell_echo_command();
        let mut handle = McpProcessRuntime::start_server(&report, &startup, &command).unwrap();

        let health = McpProcessRuntime::health(&mut handle).unwrap();
        assert_eq!(health.status, McpServerHealthStatus::Running);

        let call = McpProcessRuntime::call_tool(
            &mut registry,
            handle,
            &McpToolCallRequest {
                namespace: "mcp__team_tools".to_owned(),
                name: "query_db".to_owned(),
                mcp_name: "query-db".to_owned(),
                input: ToolInput {
                    fields: vec!["query".to_owned()],
                },
                arguments_json: r#"{"query":"ORBIT-9"}"#.to_owned(),
            },
        )
        .unwrap();

        assert!(call.forwarded_request_json.contains(MCP_TOOL_CALL_METHOD));
        assert!(call.forwarded_request_json.contains(r#""name":"query-db""#));
        assert!(call.result.summary.contains("ORBIT-9"));
        assert_eq!(
            call.result.inline_output.as_deref(),
            Some(call.result_text.as_str())
        );
        assert_eq!(call.health.status, McpServerHealthStatus::Exited);
        assert!(!call.remote_trigger_allowed);
    }

    fn mcp_report() -> McpServerDefinition {
        McpServerDefinition::new("team-tools").unwrap()
    }

    fn startup_projection() -> McpStartupProjection {
        McpStartupProjection {
            server_id: "team-tools".to_owned(),
            config_ref: "mcp://team-tools/.mcp.json".to_owned(),
            status: McpServerStartupStatus::Discovered,
            server_process_started: false,
            remote_trigger_allowed: false,
            audit_summary: "MCP startup 已发现".to_owned(),
            reasons: vec![],
        }
    }

    #[cfg(windows)]
    fn shell_echo_command() -> McpServerCommand {
        McpServerCommand {
            program: "pwsh".to_owned(),
            args: vec![
                "-NoProfile".to_owned(),
                "-Command".to_owned(),
                "$request = [Console]::In.ReadToEnd(); if ($request -like '*ORBIT-9*') { Write-Output 'MCP_RESULT:ORBIT-9' } else { exit 2 }".to_owned(),
            ],
            working_dir: None,
            env: BTreeMap::new(),
            tool_timeout_ms: None,
        }
    }

    #[cfg(not(windows))]
    fn shell_echo_command() -> McpServerCommand {
        McpServerCommand {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                "request=$(cat); case \"$request\" in *ORBIT-9*) printf 'MCP_RESULT:ORBIT-9\n' ;; *) exit 2 ;; esac".to_owned(),
            ],
            working_dir: None,
            env: BTreeMap::new(),
            tool_timeout_ms: None,
        }
    }
}
