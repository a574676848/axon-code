//! 持久 MCP stdio 会话 runtime。
//!
//! 与 [`crate::mcp_session`]（握手后即关闭进程）和
//! [`crate::mcp_process`]（spawn→写一条→wait 一次性模型）不同，本模块
//! 在 initialize / initialized / tools/list 握手后**保活子进程**，并通过独立的
//! 读取线程把 server 的每条 JSON-RPC 消息投递到通道，从而支持多次请求-响应
//! ID 关联的 `tools/call`、带超时读取、乱序响应缓冲，并缓存 tools/list 结果供
//! 后续复用。这是对齐 Codex 持久 MCP 客户端的核心能力。

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus};
use std::sync::Arc;
use std::sync::{
    Mutex,
    mpsc::{self, Receiver, RecvTimeoutError},
};
use std::thread;
use std::time::{Duration, Instant};

use axon_domain::{AxonError, AxonResult, McpServerDefinition};
use serde_json::{Value, json};

use crate::{
    MCP_INITIALIZE_METHOD, MCP_INITIALIZED_NOTIFICATION_METHOD, MCP_TOOL_CALL_METHOD,
    MCP_TOOLS_LIST_METHOD, MCP_TRANSPORT_JSONRPC_VERSION, MCP_TRANSPORT_PROTOCOL_VERSION,
    McpClientCapabilitiesRuntime, McpServerCommand, McpStartupProjection, McpToolAnnotations,
    McpToolCallResultProjection,
    mcp_stdio_launcher::{McpStdioChild, spawn_mcp_stdio_child},
    mcp_transport::{tool_annotations, tool_call_result_projection},
    runtime_limits::MCP_HANDSHAKE_TIMEOUT_SECS,
};

/// 持久会话握手完成原因。
pub const MCP_PERSISTENT_REASON_HANDSHAKE_OK: &str = "mcp_persistent:handshake_ok";
/// 持久会话保活原因。
pub const MCP_PERSISTENT_REASON_SESSION_ALIVE: &str = "mcp_persistent:session_alive";
/// 持久会话 tools/call 成功原因。
pub const MCP_PERSISTENT_REASON_TOOL_CALLED: &str = "mcp_persistent:tool_called";
/// 持久会话远控触发保持关闭原因。
pub const MCP_PERSISTENT_REASON_REMOTE_TRIGGER_CLOSED: &str =
    "mcp_persistent:remote_trigger_closed";

const MCP_INITIALIZE_ID: u64 = 1;
const MCP_TOOLS_LIST_ID: u64 = 2;
/// tools/call 起始 ID（避开握手保留 ID）。
const MCP_FIRST_CALL_ID: u64 = 100;
const MCP_CLIENT_NAME: &str = "axon-mcp";
const MCP_CLIENT_VERSION: &str = "0.1.0";
const MCP_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(50);
const MCP_STDERR_TAIL_LIMIT: usize = 4096;

/// MCP 调用的协作式取消谓词。
pub type McpCancelToken = Arc<dyn Fn() -> bool + Send + Sync + 'static>;

/// 已发现的 MCP 工具描述。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpDiscoveredTool {
    /// 工具名称。
    pub name: String,
    /// 工具描述。
    pub description: String,
    /// MCP server 暴露的完整 inputSchema。
    pub input_schema: Value,
    /// MCP server 暴露的完整 outputSchema。
    pub output_schema: Option<Value>,
    /// MCP tool 安全注解。
    ///
    /// 与 `mcp_transport` 的发现路径保持同一份解析，避免持久会话路径丢失注解后
    /// 把工具一律降级为 `Exclusive` 并发策略。
    pub annotations: McpToolAnnotations,
}

/// 持久会话握手报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPersistentHandshake {
    /// 插件名称。
    pub server_id: String,
    /// 进程 ID。
    pub process_id: u32,
    /// 已发现的工具列表（来自 server 的 tools/list 响应）。
    pub tools: Vec<McpDiscoveredTool>,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 持久会话 tools/call 结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpPersistentCallResult {
    /// JSON-RPC 请求 ID。
    pub request_id: u64,
    /// server 返回的原始结果文本（result 字段序列化）。
    pub result_text: String,
    /// server 返回的 result 安全投影。
    pub result_projection: McpToolCallResultProjection,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// 持久 MCP stdio 会话句柄。保活子进程，支持多次 tools/call。
pub struct McpPersistentSession {
    server_id: String,
    process_id: u32,
    child: McpStdioChild,
    stdin: ChildStdin,
    /// 读取线程投递的已解析 JSON-RPC 消息。
    inbox: Receiver<Value>,
    /// 乱序到达、ID 不匹配的响应缓冲，按 ID 暂存。
    pending: HashMap<u64, Value>,
    /// 子进程 stderr 的有界尾部，供启动/握手失败诊断。
    stderr_tail: Arc<Mutex<String>>,
    tools: Vec<McpDiscoveredTool>,
    next_call_id: u64,
}

/// 持久 MCP 会话 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpPersistentRuntime;

impl McpPersistentRuntime {
    /// 启动 MCP server 并完成握手，返回保活的持久会话。
    ///
    /// # Errors
    /// 能力校验失败、进程启动失败、握手超时或响应非法时返回 [`AxonError`]。
    pub fn connect(
        report: &McpServerDefinition,
        startup: &McpStartupProjection,
        command: &McpServerCommand,
    ) -> AxonResult<McpPersistentSession> {
        ensure_can_connect(report, startup, command)?;
        let mut child = spawn_child(command)?;
        let stdin = child.stdin.take().ok_or(AxonError::InvalidMcpDefinition(
            "MCP persistent stdin 不可用",
        ))?;
        let stdout = child.stdout.take().ok_or(AxonError::InvalidMcpDefinition(
            "MCP persistent stdout 不可用",
        ))?;
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_tail_reader(stderr, Arc::clone(&stderr_tail));
        }
        let inbox = spawn_reader_thread(stdout);
        let process_id = child.id();

        let mut session = McpPersistentSession {
            server_id: report.server_id.clone(),
            process_id,
            child,
            stdin,
            inbox,
            pending: HashMap::new(),
            stderr_tail,
            tools: Vec::new(),
            next_call_id: MCP_FIRST_CALL_ID,
        };

        session.perform_handshake()?;
        Ok(session)
    }
}

impl McpPersistentSession {
    /// 执行 initialize / initialized / tools/list 握手。
    fn perform_handshake(&mut self) -> AxonResult<()> {
        self.write_message(&initialize_request())?;
        let initialize_response = self.wait_for_id(
            MCP_INITIALIZE_ID,
            Some(Duration::from_secs(MCP_HANDSHAKE_TIMEOUT_SECS)),
            None,
        )?;
        if initialize_response.get("result").is_none() {
            return Err(AxonError::InvalidMcpDefinition(
                "MCP initialize 未返回 result",
            ));
        }

        self.write_message(&initialized_notification())?;

        let mut cursor = None::<String>;
        let mut seen_cursors = BTreeSet::new();
        loop {
            self.write_message(&tools_list_request(cursor.as_deref()))?;
            let tools_response = self.wait_for_id(
                MCP_TOOLS_LIST_ID,
                Some(Duration::from_secs(MCP_HANDSHAKE_TIMEOUT_SECS)),
                None,
            )?;
            self.tools.extend(parse_tools(&tools_response));
            let Some(next_cursor) = next_cursor(&tools_response) else {
                break;
            };
            if !seen_cursors.insert(next_cursor.clone()) {
                return Err(AxonError::InvalidMcpDefinition(
                    "MCP persistent tools/list 返回重复 cursor",
                ));
            }
            cursor = Some(next_cursor);
        }
        Ok(())
    }

    /// 返回握手报告。
    #[must_use]
    pub fn handshake(&self) -> McpPersistentHandshake {
        McpPersistentHandshake {
            server_id: self.server_id.clone(),
            process_id: self.process_id,
            tools: self.tools.clone(),
            reasons: vec![
                MCP_PERSISTENT_REASON_HANDSHAKE_OK.to_owned(),
                MCP_PERSISTENT_REASON_SESSION_ALIVE.to_owned(),
                MCP_PERSISTENT_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
            ],
        }
    }

    /// 返回已发现的工具列表。
    #[must_use]
    pub fn tools(&self) -> &[McpDiscoveredTool] {
        &self.tools
    }

    /// 进程是否仍存活。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// 在持久会话上调用工具，复用同一进程完成请求-响应往返。
    ///
    /// # Errors
    /// 参数非 JSON、写入失败、显式超时、server 断开或 server 返回 error 时返回 AxonError。
    pub fn call_tool(
        &mut self,
        tool_name: &str,
        arguments_json: &str,
        timeout: Option<Duration>,
        cancel_token: Option<&McpCancelToken>,
    ) -> AxonResult<McpPersistentCallResult> {
        let request_id = self.next_call_id;
        self.next_call_id += 1;

        let arguments = parse_arguments(arguments_json)?;
        let request = json!({
            "jsonrpc": MCP_TRANSPORT_JSONRPC_VERSION,
            "id": request_id,
            "method": MCP_TOOL_CALL_METHOD,
            "params": { "name": tool_name, "arguments": arguments },
        });
        self.write_message(&request)?;

        let response = self.wait_for_id(request_id, timeout, cancel_token)?;
        if response.get("error").is_some() {
            return Err(AxonError::tool_invalid_definition(
                "MCP tools/call 返回 error",
            ));
        }
        let result_text = response
            .get("result")
            .map(ToString::to_string)
            .unwrap_or_default();
        let result_projection = tool_call_result_projection(response.get("result"));

        Ok(McpPersistentCallResult {
            request_id,
            result_text,
            result_projection,
            reasons: vec![
                MCP_PERSISTENT_REASON_TOOL_CALLED.to_owned(),
                MCP_PERSISTENT_REASON_REMOTE_TRIGGER_CLOSED.to_owned(),
            ],
        })
    }

    /// 主动关闭会话：尝试优雅结束子进程。
    pub fn shutdown(mut self) {
        self.terminate();
    }

    fn write_message(&mut self, message: &Value) -> AxonResult<()> {
        let serialized = serde_json::to_string(message)
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP persistent 请求序列化失败"))?;
        self.stdin
            .write_all(serialized.as_bytes())
            .and_then(|()| self.stdin.write_all(b"\n"))
            .and_then(|()| self.stdin.flush())
            .map_err(|_| AxonError::InvalidMcpDefinition("MCP persistent 请求写入失败"))
    }

    /// 等待与 id 匹配的响应；仅在传入显式超时时限制等待，乱序响应进入缓冲。
    fn wait_for_id(
        &mut self,
        id: u64,
        timeout: Option<Duration>,
        cancel_token: Option<&McpCancelToken>,
    ) -> AxonResult<Value> {
        if let Some(value) = self.pending.remove(&id) {
            return Ok(value);
        }
        let deadline = timeout.map(|value| Instant::now() + value);
        loop {
            if cancel_token.is_some_and(|is_cancelled| is_cancelled()) {
                self.terminate();
                return Err(AxonError::InvalidMcpDefinition("MCP persistent 调用已取消"));
            }
            let wait = next_wait_duration(deadline, cancel_token.is_some())?;
            let value = match wait {
                Some(wait) => match self.inbox.recv_timeout(wait) {
                    Ok(value) => value,
                    Err(RecvTimeoutError::Timeout) if cancel_token.is_some() => continue,
                    Err(RecvTimeoutError::Timeout) => {
                        return Err(AxonError::InvalidMcpDefinition("MCP persistent 响应超时"));
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        return Err(self.process_closed_error(id));
                    }
                },
                None => match self.inbox.recv() {
                    Ok(value) => value,
                    Err(_) => return Err(self.process_closed_error(id)),
                },
            };
            match value.get("id").and_then(Value::as_u64) {
                Some(found) if found == id => return Ok(value),
                // 携带其他 ID 的响应缓冲，留待后续调用消费。
                Some(other) => {
                    self.pending.insert(other, value);
                }
                // 通知（无 id）忽略。
                None => {}
            }
        }
    }

    fn terminate(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn process_closed_error(&mut self, waiting_for_id: u64) -> AxonError {
        let status = self.child.try_wait().ok().flatten();
        let stderr = self
            .stderr_tail
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let status_text = status
            .as_ref()
            .map(format_exit_status)
            .unwrap_or_else(|| "unknown".to_owned());
        let stderr_text = if stderr.trim().is_empty() {
            "<empty>".to_owned()
        } else {
            sanitize_stderr(&stderr)
        };
        AxonError::runtime_failure(format!(
            "MCP persistent 进程在响应前关闭：pid={} waiting_for_id={} exit_status={} stderr_tail={}",
            self.process_id, waiting_for_id, status_text, stderr_text
        ))
    }
}

fn next_wait_duration(
    deadline: Option<Instant>,
    cancellable: bool,
) -> AxonResult<Option<Duration>> {
    let remaining = deadline
        .map(|value| {
            value
                .checked_duration_since(Instant::now())
                .ok_or(AxonError::InvalidMcpDefinition("MCP persistent 响应超时"))
        })
        .transpose()?;
    Ok(match (remaining, cancellable) {
        (Some(remaining), true) => Some(remaining.min(MCP_CANCEL_POLL_INTERVAL)),
        (Some(remaining), false) => Some(remaining),
        (None, true) => Some(MCP_CANCEL_POLL_INTERVAL),
        (None, false) => None,
    })
}

impl Drop for McpPersistentSession {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn spawn_stderr_tail_reader(stderr: ChildStderr, tail: Arc<Mutex<String>>) {
    let _ = thread::Builder::new()
        .name("axon-mcp-stderr-reader".to_owned())
        .spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let mut current = tail
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        current.push_str(&line);
                        truncate_tail(&mut current, MCP_STDERR_TAIL_LIMIT);
                    }
                }
            }
        });
}

fn truncate_tail(value: &mut String, limit: usize) {
    if value.len() <= limit {
        return;
    }
    let excess = value.len() - limit;
    let boundary = value
        .char_indices()
        .find_map(|(index, _)| (index >= excess).then_some(index))
        .unwrap_or(value.len());
    value.drain(..boundary);
}

fn format_exit_status(status: &ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("code:{code}"),
        None => "signal_or_unknown".to_owned(),
    }
}

fn sanitize_stderr(stderr: &str) -> String {
    stderr
        .lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if ["token", "authorization", "api_key", "apikey", "secret"]
                .iter()
                .any(|marker| lower.contains(marker))
            {
                "[stderr 敏感行已脱敏]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\\n")
}

/// 启动读取线程：持续读取 server stdout，把每条 JSON-RPC 消息投递到通道。
fn spawn_reader_thread(stdout: ChildStdout) -> Receiver<Value> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
                        && sender.send(value).is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
    receiver
}

fn ensure_can_connect(
    report: &McpServerDefinition,
    startup: &McpStartupProjection,
    command: &McpServerCommand,
) -> AxonResult<()> {
    debug_assert!(!report.server_id.is_empty());
    if startup.server_process_started || startup.remote_trigger_allowed {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP persistent 会话只能由本机 runtime 启动",
        ));
    }
    if command.program.trim().is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP persistent 启动命令不能为空",
        ));
    }
    Ok(())
}

fn spawn_child(command: &McpServerCommand) -> AxonResult<McpStdioChild> {
    spawn_mcp_stdio_child(command)
}

fn parse_tools(response: &Value) -> Vec<McpDiscoveredTool> {
    response
        .get("result")
        .and_then(|result| result.get("tools"))
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let name = tool.get("name").and_then(Value::as_str)?;
                    let description = tool
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some(McpDiscoveredTool {
                        name: name.to_owned(),
                        description: description.to_owned(),
                        input_schema: tool
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(crate::mcp_tools::empty_mcp_input_schema),
                        output_schema: tool.get("outputSchema").cloned(),
                        annotations: tool_annotations(tool),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_arguments(arguments_json: &str) -> AxonResult<Value> {
    if arguments_json.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(arguments_json)
        .map_err(|_| AxonError::tool_invalid_definition("MCP tool arguments 必须是 JSON"))
}

fn initialize_request() -> Value {
    json!({
        "jsonrpc": MCP_TRANSPORT_JSONRPC_VERSION,
        "id": MCP_INITIALIZE_ID,
        "method": MCP_INITIALIZE_METHOD,
        "params": {
            "protocolVersion": MCP_TRANSPORT_PROTOCOL_VERSION,
            "capabilities": McpClientCapabilitiesRuntime::initialize_capabilities_value(),
            "clientInfo": { "name": MCP_CLIENT_NAME, "version": MCP_CLIENT_VERSION },
        }
    })
}

fn initialized_notification() -> Value {
    json!({
        "jsonrpc": MCP_TRANSPORT_JSONRPC_VERSION,
        "method": MCP_INITIALIZED_NOTIFICATION_METHOD,
        "params": {},
    })
}

fn tools_list_request(cursor: Option<&str>) -> Value {
    let params = cursor.map_or_else(
        || json!({}),
        |cursor| {
            json!({
                "cursor": cursor,
            })
        },
    );
    json!({
        "jsonrpc": MCP_TRANSPORT_JSONRPC_VERSION,
        "id": MCP_TOOLS_LIST_ID,
        "method": MCP_TOOLS_LIST_METHOD,
        "params": params,
    })
}

fn next_cursor(response: &Value) -> Option<String> {
    response
        .get("result")
        .and_then(|result| result.get("nextCursor"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|cursor| !cursor.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
#[path = "persistent/tests.rs"]
mod tests;
