//! 插件 MCP stdio 会话握手 runtime。

use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::{sync::mpsc, thread, time::Duration};

use axon_domain::{AxonError, AxonResult, McpServerDefinition};
use serde_json::{Value, json};

use crate::{
    MCP_TRANSPORT_PROTOCOL_VERSION, McpClientCapabilitiesRuntime, McpNotificationEvent,
    McpNotificationSummary, McpServerCommand, McpServerHealth, McpServerHealthStatus,
    McpStartupProjection, ToolRegistry,
    mcp_notifications::notification_event_from_value,
    mcp_stdio_launcher::{McpStdioChild, spawn_mcp_stdio_child},
    mcp_transport::tool_annotations,
};

/// MCP initialize 方法名。
pub const MCP_INITIALIZE_METHOD: &str = "initialize";
/// MCP initialized 通知方法名。
pub const MCP_INITIALIZED_NOTIFICATION_METHOD: &str = "notifications/initialized";
/// MCP tools/list 方法名。
pub const MCP_TOOLS_LIST_METHOD: &str = "tools/list";
/// stdio initialize 成功原因。
pub const MCP_SESSION_REASON_INITIALIZED: &str = "mcp_session:initialized";
/// stdio tool list 握手成功原因。
pub const MCP_SESSION_REASON_TOOLS_LISTED: &str = "mcp_session:tools_listed";
/// stdio 会话崩溃恢复原因。
pub const MCP_SESSION_REASON_CRASH_RECOVERY: &str = "mcp_session:crash_recovery";
const MCP_INITIALIZE_ID: u64 = 1;
const MCP_TOOLS_LIST_ID: u64 = 2;
const MCP_CLIENT_NAME: &str = "axon-mcp";
const MCP_CLIENT_VERSION: &str = "0.1.0";
const MCP_META_PROTOCOL_VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const MCP_META_CLIENT_INFO_KEY: &str = "io.modelcontextprotocol/clientInfo";
const MCP_STDIO_POST_INITIALIZED_DRAIN_MS: u64 = 50;

/// 插件 MCP 崩溃恢复决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpCrashRecovery {
    /// 是否检测到崩溃或握手失败。
    pub crash_detected: bool,
    /// 是否建议本机 runtime 重启。
    pub restart_recommended: bool,
    /// terminal 主循环是否继续运行。
    pub terminal_main_loop_continues: bool,
    /// 是否允许远控触发恢复。
    pub remote_trigger_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 插件 MCP stdio 会话握手报告。
#[derive(Debug, Clone, PartialEq)]
pub struct McpStdioSessionReport {
    /// 插件名称。
    pub server_id: String,
    /// 启动的进程 ID。
    pub process_id: u32,
    /// initialize 是否完成。
    pub discovered: bool,
    /// tools/list 是否完成。
    pub tools_listed: bool,
    /// server 返回的 tool 名称。
    pub listed_tools: Vec<String>,
    /// server 返回的 tool schema 摘要。
    pub tool_schemas: Vec<crate::McpToolSchema>,
    /// tools/list 下一页 cursor。
    pub tools_next_cursor: Option<String>,
    /// 已执行 tools/list 请求数量。
    pub tools_requests_executed: usize,
    /// 发送到 stdin 的 JSON-RPC 请求。
    pub handshake_requests: Vec<String>,
    /// 健康投影。
    pub health: McpServerHealth,
    /// 崩溃恢复决策。
    pub recovery: McpCrashRecovery,
    /// MCP server 通知摘要。
    pub notification_summary: McpNotificationSummary,
}

/// 插件 MCP stdio 请求交换报告。
#[derive(Debug, Clone, PartialEq)]
pub struct McpStdioExchangeReport {
    /// JSON-RPC 响应。
    pub responses: Vec<Value>,
    /// MCP server 通知事件。
    pub notifications: Vec<McpNotificationEvent>,
    /// MCP server 通知摘要。
    pub notification_summary: McpNotificationSummary,
}

/// 插件 MCP stdio 会话 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpStdioSessionRuntime;

/// 插件 MCP stdio 请求交换 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpStdioExchangeRuntime;

impl McpStdioSessionRuntime {
    /// 启动 MCP server，并完成 initialize / initialized / tools/list 握手。
    pub fn discover_and_list_tools(
        report: &McpServerDefinition,
        startup: &McpStartupProjection,
        command: &McpServerCommand,
        registry: &ToolRegistry,
    ) -> AxonResult<McpStdioSessionReport> {
        ensure_stdio_session_can_start(report, startup, command)?;
        let requests = handshake_requests(registry)?;
        let mut child = spawn_mcp_stdio_child(command)?;
        let process_id = child.id();
        let Some(mut stdin) = child.stdin.take() else {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP stdio server stdin 不可用",
            ));
        };
        for request in &requests {
            stdin
                .write_all(request.as_bytes())
                .and_then(|()| stdin.write_all(b"\n"))
                .map_err(|_| AxonError::InvalidMcpDefinition("MCP stdio 握手写入失败"))?;
        }
        let stdout = child.stdout.take().ok_or(AxonError::InvalidMcpDefinition(
            "MCP stdio server stdout 不可用",
        ))?;
        let parsed = read_stdio_handshake(stdout, stdin)?;
        let running = if parsed.timed_out {
            false
        } else {
            child
                .try_wait()
                .map_err(|_| AxonError::InvalidMcpDefinition("MCP stdio 健康检查失败"))?
                .is_none()
        };
        let success = !parsed.timed_out && running && parsed.discovered && parsed.tools_listed;
        let cleanup_exit_code = cleanup_child(child);
        let health_status = if success {
            McpServerHealthStatus::Running
        } else {
            McpServerHealthStatus::Exited
        };
        let recovery = recovery_decision(success);

        Ok(McpStdioSessionReport {
            server_id: report.server_id.clone(),
            process_id,
            discovered: parsed.discovered,
            tools_listed: parsed.tools_listed,
            listed_tools: parsed.tools,
            tool_schemas: parsed.tool_schemas,
            tools_next_cursor: parsed.tools_next_cursor,
            tools_requests_executed: parsed.tools_requests_executed,
            handshake_requests: requests,
            health: McpServerHealth {
                server_id: report.server_id.clone(),
                process_id,
                status: health_status,
                exit_code: (!success).then_some(cleanup_exit_code).flatten(),
                remote_trigger_allowed: false,
                reasons: health_reasons(success, &startup.config_ref),
            },
            recovery,
            notification_summary: McpNotificationSummary::from_events(&parsed.notifications),
        })
    }
}

impl McpStdioExchangeRuntime {
    /// 启动 stdio MCP server，完成 initialize/initialized 后发送指定请求并返回响应。
    ///
    /// # Errors
    /// 进程启动、stdin/stdout、请求写入或响应读取失败时返回错误。
    pub fn exchange(command: &McpServerCommand, requests: Vec<String>) -> AxonResult<Vec<Value>> {
        Self::exchange_with_notifications(command, requests).map(|report| report.responses)
    }

    /// 启动 stdio MCP server，完成请求并返回响应与通知投影。
    ///
    /// # Errors
    /// 进程启动、stdin/stdout、请求写入或响应读取失败时返回错误。
    pub fn exchange_with_notifications(
        command: &McpServerCommand,
        requests: Vec<String>,
    ) -> AxonResult<McpStdioExchangeReport> {
        if command.program.trim().is_empty() {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP stdio server 启动命令不能为空",
            ));
        }
        let mut child = spawn_mcp_stdio_child(command)?;
        let Some(mut stdin) = child.stdin.take() else {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP stdio server stdin 不可用",
            ));
        };
        let initialize = serde_json::to_string(&initialize_request_value())
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP initialize 序列化失败"))?;
        let initialized = serde_json::to_string(&initialized_notification_value())
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP initialized 序列化失败"))?;
        let expected_responses = requests.len() + 1;
        write_stdio_request(&mut stdin, &initialize)?;

        let stdout = child.stdout.take().ok_or(AxonError::InvalidMcpDefinition(
            "MCP stdio server stdout 不可用",
        ))?;
        let report =
            read_stdio_responses(stdout, stdin, initialized, requests, expected_responses)?;
        let _ = cleanup_child(child);
        Ok(report)
    }
}

struct ParsedHandshake {
    discovered: bool,
    tools_listed: bool,
    tools: Vec<String>,
    tool_schemas: Vec<crate::McpToolSchema>,
    tools_next_cursor: Option<String>,
    tools_requests_executed: usize,
    notifications: Vec<McpNotificationEvent>,
    timed_out: bool,
}

fn ensure_stdio_session_can_start(
    report: &McpServerDefinition,
    startup: &McpStartupProjection,
    command: &McpServerCommand,
) -> AxonResult<()> {
    debug_assert!(!report.server_id.is_empty());
    if startup.server_process_started || startup.remote_trigger_allowed {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP stdio 会话只能由本机 runtime 启动",
        ));
    }
    if command.program.trim().is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP stdio server 启动命令不能为空",
        ));
    }
    Ok(())
}

fn handshake_requests(registry: &ToolRegistry) -> AxonResult<Vec<String>> {
    let initialize = initialize_request_value();
    let initialized = initialized_notification_value();
    let tools = registry
        .model_tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.qualified_name,
                "description": tool.description,
                "schemaHash": tool.schema_hash
            })
        })
        .collect::<Vec<_>>();
    let list = json!({
        "jsonrpc": "2.0",
        "id": MCP_TOOLS_LIST_ID,
        "method": MCP_TOOLS_LIST_METHOD,
        "params": {
            "expectedTools": tools,
            "_meta": mcp_meta_value()
        }
    });
    Ok(vec![
        serde_json::to_string(&initialize)
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP initialize 序列化失败"))?,
        serde_json::to_string(&initialized)
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP initialized 序列化失败"))?,
        serde_json::to_string(&list)
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP tools/list 序列化失败"))?,
    ])
}

fn initialize_request_value() -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": MCP_INITIALIZE_ID,
        "method": MCP_INITIALIZE_METHOD,
        "params": {
            "protocolVersion": MCP_TRANSPORT_PROTOCOL_VERSION,
            "capabilities": McpClientCapabilitiesRuntime::initialize_capabilities_value(),
            "clientInfo": {
                "name": MCP_CLIENT_NAME,
                "version": MCP_CLIENT_VERSION
            },
            "_meta": mcp_meta_value()
        }
    })
}

fn initialized_notification_value() -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": MCP_INITIALIZED_NOTIFICATION_METHOD,
        "params": {
            "_meta": mcp_meta_value()
        }
    })
}

fn mcp_meta_value() -> Value {
    json!({
        MCP_META_PROTOCOL_VERSION_KEY: MCP_TRANSPORT_PROTOCOL_VERSION,
        MCP_META_CLIENT_INFO_KEY: {
            "name": MCP_CLIENT_NAME,
            "version": MCP_CLIENT_VERSION
        }
    })
}

fn read_stdio_handshake(
    stdout: std::process::ChildStdout,
    stdin: std::process::ChildStdin,
) -> AxonResult<ParsedHandshake> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let parsed = read_stdio_handshake_blocking(stdout, stdin);
        let _ = sender.send(parsed);
    });
    match receiver.recv_timeout(Duration::from_secs(
        crate::runtime_limits::MCP_HANDSHAKE_TIMEOUT_SECS,
    )) {
        Ok(parsed) => parsed,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(ParsedHandshake {
            discovered: false,
            tools_listed: false,
            tools: Vec::new(),
            tool_schemas: Vec::new(),
            tools_next_cursor: None,
            tools_requests_executed: 0,
            notifications: Vec::new(),
            timed_out: true,
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(AxonError::InvalidMcpDefinition(
            "MCP stdio 响应读取线程异常退出",
        )),
    }
}

fn read_stdio_responses(
    stdout: std::process::ChildStdout,
    stdin: std::process::ChildStdin,
    initialized: String,
    requests: Vec<String>,
    max_responses: usize,
) -> AxonResult<McpStdioExchangeReport> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let parsed =
            read_stdio_responses_blocking(stdout, stdin, initialized, requests, max_responses);
        let _ = sender.send(parsed);
    });
    match receiver.recv_timeout(Duration::from_secs(
        crate::runtime_limits::MCP_HANDSHAKE_TIMEOUT_SECS,
    )) {
        Ok(parsed) => parsed,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(AxonError::InvalidMcpDefinition(
            "MCP stdio resource 响应超时",
        )),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(AxonError::InvalidMcpDefinition(
            "MCP stdio resource 响应读取线程异常退出",
        )),
    }
}

fn read_stdio_responses_blocking(
    stdout: std::process::ChildStdout,
    mut stdin: std::process::ChildStdin,
    initialized: String,
    requests: Vec<String>,
    max_responses: usize,
) -> AxonResult<McpStdioExchangeReport> {
    let mut responses = Vec::new();
    let mut notifications = Vec::new();
    let mut pending_requests = VecDeque::from(requests);
    let mut initialized_sent = false;
    let mut business_started = false;
    let line_receiver = spawn_stdio_line_reader(stdout);
    while responses.len() < max_responses {
        let line_result = if initialized_sent && !business_started {
            match line_receiver
                .recv_timeout(Duration::from_millis(MCP_STDIO_POST_INITIALIZED_DRAIN_MS))
            {
                Ok(line_result) => line_result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    write_next_business_request(
                        &mut stdin,
                        &mut pending_requests,
                        &mut business_started,
                    )?;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match line_receiver.recv() {
                Ok(line_result) => line_result,
                Err(_) => break,
            }
        };
        let line =
            line_result.map_err(|_| AxonError::InvalidMcpDefinition("MCP stdio 响应读取失败"))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(event) = notification_event_from_value(&value) {
            if let Some(response) = event.denial_response_json.as_deref() {
                write_stdio_request(&mut stdin, response)
                    .map_err(|_| AxonError::InvalidMcpDefinition("MCP stdio 拒绝响应写入失败"))?;
            }
            notifications.push(event);
        } else if value.get("id").is_some() {
            responses.push(value);
            if initialized_sent {
                write_next_business_request(
                    &mut stdin,
                    &mut pending_requests,
                    &mut business_started,
                )?;
            } else {
                write_stdio_request(&mut stdin, &initialized)?;
                initialized_sent = true;
            }
        }
    }
    let notification_summary = McpNotificationSummary::from_events(&notifications);
    Ok(McpStdioExchangeReport {
        responses,
        notifications,
        notification_summary,
    })
}

fn spawn_stdio_line_reader(
    stdout: std::process::ChildStdout,
) -> mpsc::Receiver<Result<String, std::io::Error>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let should_stop = line.is_err();
            let _ = sender.send(line);
            if should_stop {
                break;
            }
        }
    });
    receiver
}

fn write_next_business_request(
    stdin: &mut std::process::ChildStdin,
    pending_requests: &mut VecDeque<String>,
    business_started: &mut bool,
) -> AxonResult<()> {
    if let Some(request) = pending_requests.pop_front() {
        write_stdio_request(stdin, &request)?;
        *business_started = true;
    }
    Ok(())
}

fn write_stdio_request(stdin: &mut std::process::ChildStdin, request: &str) -> AxonResult<()> {
    stdin
        .write_all(request.as_bytes())
        .and_then(|()| stdin.write_all(b"\n"))
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP stdio 请求写入失败"))
}

fn read_stdio_handshake_blocking(
    stdout: std::process::ChildStdout,
    mut stdin: std::process::ChildStdin,
) -> AxonResult<ParsedHandshake> {
    let mut discovered = false;
    let mut tools_listed = false;
    let mut tools = Vec::new();
    let mut tool_schemas = Vec::new();
    let mut notifications = Vec::new();
    let mut tools_next_cursor = None::<String>;
    let mut tools_requests_executed = 0;
    let mut seen_cursors = BTreeSet::new();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.map_err(|_| AxonError::InvalidMcpDefinition("MCP stdio 响应读取失败"))?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(event) = notification_event_from_value(&value) {
            notifications.push(event);
            continue;
        }
        if value.get("id").and_then(Value::as_u64) == Some(MCP_INITIALIZE_ID)
            && value.get("result").is_some()
        {
            discovered = true;
        }
        if value.get("id").and_then(Value::as_u64) == Some(MCP_TOOLS_LIST_ID) {
            let returned_tools = value
                .get("result")
                .and_then(|result| result.get("tools"))
                .and_then(Value::as_array)
                .ok_or(AxonError::InvalidMcpDefinition(
                    "MCP tools/list 响应缺少 tools",
                ))?;
            tools_listed = true;
            tools_requests_executed += 1;
            for tool in returned_tools {
                append_tool_schema(tool, &mut tools, &mut tool_schemas);
            }
            tools_next_cursor = next_cursor(&value);
            if let Some(cursor) = &tools_next_cursor
                && !seen_cursors.insert(cursor.clone())
            {
                return Err(AxonError::InvalidMcpDefinition(
                    "MCP tools/list 返回重复 cursor",
                ));
            }
            if let Some(cursor) = tools_next_cursor.as_deref() {
                let request = tools_list_cursor_request(cursor)?;
                write_stdio_request(&mut stdin, &request)?;
                continue;
            }
        }
        if discovered && tools_listed && tools_next_cursor.is_none() {
            break;
        }
    }
    Ok(ParsedHandshake {
        discovered,
        tools_listed,
        tools,
        tool_schemas,
        tools_next_cursor,
        tools_requests_executed,
        notifications,
        timed_out: false,
    })
}

fn append_tool_schema(
    tool: &Value,
    tools: &mut Vec<String>,
    tool_schemas: &mut Vec<crate::McpToolSchema>,
) {
    let Some(name) = tool.get("name").and_then(Value::as_str) else {
        return;
    };
    tools.push(name.to_owned());
    tool_schemas.push(crate::McpToolSchema {
        name: name.to_owned(),
        mcp_name: name.to_owned(),
        description: tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        input_schema: tool
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(crate::mcp_tools::empty_mcp_input_schema),
        output_schema: tool.get("outputSchema").cloned(),
        annotations: tool_annotations(tool),
    });
}

fn next_cursor(value: &Value) -> Option<String> {
    value
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn tools_list_cursor_request(cursor: &str) -> AxonResult<String> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": MCP_TOOLS_LIST_ID,
        "method": MCP_TOOLS_LIST_METHOD,
        "params": {
            "cursor": cursor,
            "_meta": mcp_meta_value()
        }
    }))
    .map_err(|_| AxonError::InvalidMcpDefinition("MCP tools/list cursor 序列化失败"))
}

fn cleanup_child(mut child: McpStdioChild) -> Option<i32> {
    match child.try_wait() {
        Ok(Some(status)) => status.code(),
        Ok(None) => {
            let _ = child.kill();
            child.wait().ok().and_then(|status| status.code())
        }
        Err(_) => None,
    }
}

fn recovery_decision(success: bool) -> McpCrashRecovery {
    if success {
        return McpCrashRecovery {
            crash_detected: false,
            restart_recommended: false,
            terminal_main_loop_continues: true,
            remote_trigger_allowed: false,
            reasons: vec![
                MCP_SESSION_REASON_INITIALIZED.to_owned(),
                MCP_SESSION_REASON_TOOLS_LISTED.to_owned(),
            ],
        };
    }
    McpCrashRecovery {
        crash_detected: true,
        restart_recommended: true,
        terminal_main_loop_continues: true,
        remote_trigger_allowed: false,
        reasons: vec![MCP_SESSION_REASON_CRASH_RECOVERY.to_owned()],
    }
}

fn health_reasons(success: bool, config_ref: &str) -> Vec<String> {
    if success {
        vec![
            MCP_SESSION_REASON_INITIALIZED.to_owned(),
            MCP_SESSION_REASON_TOOLS_LISTED.to_owned(),
            config_ref.to_owned(),
        ]
    } else {
        vec![
            MCP_SESSION_REASON_CRASH_RECOVERY.to_owned(),
            config_ref.to_owned(),
        ]
    }
}
