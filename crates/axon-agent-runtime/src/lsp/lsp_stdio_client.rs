//! 通用 LSP stdio client 最小骨架。

use std::collections::HashMap;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use axon_domain::{AxonError, AxonResult};
use serde_json::{Value, json};

use crate::LspLaunchPlan;
use crate::lsp_document::canonical_file_path_from_uri;
use crate::runtime_limits::{
    LSP_MESSAGE_QUEUE_CAPACITY, LSP_PUSH_DIAGNOSTICS_SETTLE_MS, LSP_PUSH_DIAGNOSTICS_WAIT_MS,
    LSP_RESPONSE_TIMEOUT_SECS,
};
use axon_process::configure_non_interactive_process;

#[path = "lsp_stdio_diagnostics.rs"]
mod lsp_stdio_diagnostics;
#[path = "lsp_stdio_file_sync.rs"]
mod lsp_stdio_file_sync;
#[path = "lsp_stdio_protocol.rs"]
mod lsp_stdio_protocol;
pub use lsp_stdio_file_sync::{LspWorkspaceFileChange, LspWorkspaceFileChangeKind};
#[cfg(test)]
use lsp_stdio_protocol::read_content_length;
use lsp_stdio_protocol::{read_message_from, workspace_name};

/// LSP JSON-RPC 版本。
pub const LSP_JSONRPC_VERSION: &str = "2.0";
/// LSP initialize 方法。
pub const LSP_INITIALIZE_METHOD: &str = "initialize";
/// LSP initialized 通知方法。
pub const LSP_INITIALIZED_NOTIFICATION_METHOD: &str = "initialized";
const LSP_INITIALIZE_ID: u64 = 1;
const LSP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(LSP_RESPONSE_TIMEOUT_SECS);
const LSP_PUSH_DIAGNOSTICS_WAIT: Duration = Duration::from_millis(LSP_PUSH_DIAGNOSTICS_WAIT_MS);
const LSP_PUSH_DIAGNOSTICS_SETTLE: Duration = Duration::from_millis(LSP_PUSH_DIAGNOSTICS_SETTLE_MS);
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;

#[derive(Debug)]
struct LspDocumentState {
    text: String,
    version: i32,
}

/// 初始化请求入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspClientInitializeInput {
    /// 工作区根 URI。
    pub root_uri: String,
    /// 当前进程 ID。
    pub process_id: u32,
}

/// 初始化响应摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspClientInitializeReport {
    /// 服务端 ID。
    pub server_id: String,
    /// 是否完成 initialize 握手。
    pub initialized: bool,
    /// 服务端返回的 capabilities。
    pub capabilities: Value,
    /// 本次发送的 JSON-RPC 消息。
    pub requests_sent: Vec<String>,
}

/// 通用 request 入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspClientRequest {
    /// JSON-RPC request id。
    pub id: u64,
    /// 方法名。
    pub method: String,
    /// JSON 参数。
    pub params: Value,
}

/// 通用 workspace symbol 入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspWorkspaceSymbolRequest {
    /// JSON-RPC request id。
    pub id: u64,
    /// 查询关键字。
    pub query: String,
}

/// 通用文档位置查询入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDocumentPositionRequest {
    /// JSON-RPC request id。
    pub id: u64,
    /// 目标文档 URI。
    pub text_document_uri: String,
    /// 行号。
    pub line: u32,
    /// 列号。
    pub character: u32,
}

/// 通用 diagnostics 入参。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspDiagnosticRequest {
    /// JSON-RPC request id。
    pub id: u64,
    /// 目标文档 URI。
    pub text_document_uri: String,
}

/// LSP stdio client。
pub struct LspStdioClient {
    server_id: String,
    root_uri: Option<String>,
    supports_pull_diagnostics: bool,
    documents: HashMap<String, LspDocumentState>,
    published_diagnostics: HashMap<PathBuf, Value>,
    child: Child,
    stdin: Option<ChildStdin>,
    incoming: Option<Receiver<AxonResult<Value>>>,
    reader_thread: Option<JoinHandle<()>>,
    stderr_thread: Option<JoinHandle<()>>,
}

impl LspStdioClient {
    /// 启动一个 LSP 子进程。
    pub fn start(plan: &LspLaunchPlan) -> AxonResult<Self> {
        let mut command = Command::new(&plan.program);
        configure_non_interactive_process(&mut command);
        command.args(&plan.args);
        command.current_dir(&plan.cwd);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| AxonError::tool_invalid_definition("LSP stdio client 启动失败"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AxonError::tool_invalid_definition("LSP stdin 不可用"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AxonError::tool_invalid_definition("LSP stdout 不可用"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| AxonError::tool_invalid_definition("LSP stderr 不可用"))?;
        let (sender, incoming) = mpsc::sync_channel(LSP_MESSAGE_QUEUE_CAPACITY);
        let reader_thread = thread::Builder::new()
            .name(format!("axon-lsp-reader-{}", plan.server_id))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let message = read_message_from(&mut reader);
                    let failed = message.is_err();
                    if sender.send(message).is_err() || failed {
                        break;
                    }
                }
            })
            .map_err(|_| AxonError::tool_invalid_definition("LSP stdout reader 启动失败"))?;
        let stderr_thread = thread::Builder::new()
            .name(format!("axon-lsp-stderr-{}", plan.server_id))
            .spawn(move || {
                let mut stderr = stderr;
                let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            })
            .map_err(|_| AxonError::tool_invalid_definition("LSP stderr reader 启动失败"))?;
        Ok(Self {
            server_id: plan.server_id.clone(),
            root_uri: None,
            supports_pull_diagnostics: false,
            documents: HashMap::new(),
            published_diagnostics: HashMap::new(),
            child,
            stdin: Some(stdin),
            incoming: Some(incoming),
            reader_thread: Some(reader_thread),
            stderr_thread: Some(stderr_thread),
        })
    }

    /// 完成 initialize / initialized 握手。
    pub fn initialize(
        &mut self,
        input: &LspClientInitializeInput,
    ) -> AxonResult<LspClientInitializeReport> {
        self.initialize_with_timeout(input, LSP_RESPONSE_TIMEOUT)
    }

    /// 在调用方给定窗口内完成 initialize / initialized 握手。
    pub(crate) fn initialize_with_timeout(
        &mut self,
        input: &LspClientInitializeInput,
        timeout: Duration,
    ) -> AxonResult<LspClientInitializeReport> {
        let initialize = json!({
            "jsonrpc": LSP_JSONRPC_VERSION,
            "id": LSP_INITIALIZE_ID,
            "method": LSP_INITIALIZE_METHOD,
            "params": {
                "processId": input.process_id,
                "rootUri": input.root_uri,
                "workspaceFolders": [{
                    "uri": input.root_uri,
                    "name": workspace_name(&input.root_uri),
                }],
                "capabilities": {
                    "workspace": {
                        "configuration": true,
                        "workspaceFolders": true,
                    },
                    "textDocument": {
                        "synchronization": {
                            "dynamicRegistration": false,
                            "didSave": true,
                        },
                        "publishDiagnostics": {
                            "relatedInformation": true,
                        },
                        "diagnostic": {
                            "dynamicRegistration": false,
                            "relatedDocumentSupport": true,
                        },
                    },
                },
            }
        });
        let initialized = json!({
            "jsonrpc": LSP_JSONRPC_VERSION,
            "method": LSP_INITIALIZED_NOTIFICATION_METHOD,
            "params": {}
        });
        let initialize_json = serde_json::to_string(&initialize)
            .map_err(|_| AxonError::tool_invalid_definition("LSP initialize 序列化失败"))?;
        let initialized_notification_json = serde_json::to_string(&initialized)
            .map_err(|_| AxonError::tool_invalid_definition("LSP initialized 序列化失败"))?;
        self.write_message(&initialize_json)?;
        let response = self.read_response_with_timeout(LSP_INITIALIZE_ID, timeout)?;
        self.write_message(&initialized_notification_json)?;
        let capabilities = response
            .get("result")
            .and_then(|result| result.get("capabilities"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        self.root_uri = Some(input.root_uri.clone());
        self.supports_pull_diagnostics = capabilities
            .get("diagnosticProvider")
            .is_some_and(|value| !value.is_null() && value != &Value::Bool(false));
        Ok(LspClientInitializeReport {
            server_id: self.server_id.clone(),
            initialized: response.get("id").and_then(Value::as_u64) == Some(LSP_INITIALIZE_ID),
            capabilities,
            requests_sent: vec![initialize_json, initialized_notification_json],
        })
    }

    /// 发送一个 JSON-RPC request 并等待同 id 响应。
    pub fn send_request(&mut self, request: &LspClientRequest) -> AxonResult<Value> {
        self.send_request_with_timeout(request, LSP_RESPONSE_TIMEOUT)
    }

    fn send_request_with_timeout(
        &mut self,
        request: &LspClientRequest,
        timeout: Duration,
    ) -> AxonResult<Value> {
        let body = json!({
            "jsonrpc": LSP_JSONRPC_VERSION,
            "id": request.id,
            "method": request.method,
            "params": request.params,
        });
        let body_json = serde_json::to_string(&body)
            .map_err(|_| AxonError::tool_invalid_definition("LSP request 序列化失败"))?;
        self.write_message(&body_json)?;
        self.read_response_with_timeout(request.id, timeout)
    }

    /// 发送一个 JSON-RPC notification。
    pub fn send_notification(&mut self, method: &str, params: &Value) -> AxonResult<()> {
        let body = json!({
            "jsonrpc": LSP_JSONRPC_VERSION,
            "method": method,
            "params": params,
        });
        let body_json = serde_json::to_string(&body)
            .map_err(|_| AxonError::tool_invalid_definition("LSP notification 序列化失败"))?;
        self.write_message(&body_json)
    }

    /// 通用 workspace symbol 查询。
    pub fn workspace_symbol(&mut self, request: &LspWorkspaceSymbolRequest) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request.id,
            method: "workspace/symbol".to_owned(),
            params: json!({ "query": request.query }),
        })
    }

    /// 执行 `textDocument/definition` 查询。
    pub fn definition(&mut self, request: &LspDocumentPositionRequest) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request.id,
            method: "textDocument/definition".to_owned(),
            params: json!({
                "textDocument": { "uri": request.text_document_uri },
                "position": { "line": request.line, "character": request.character },
            }),
        })
    }

    /// 执行 `textDocument/references` 查询。
    pub fn references(&mut self, request: &LspDocumentPositionRequest) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request.id,
            method: "textDocument/references".to_owned(),
            params: json!({
                "textDocument": { "uri": request.text_document_uri },
                "position": { "line": request.line, "character": request.character },
                "context": { "includeDeclaration": true },
            }),
        })
    }

    /// 执行 `textDocument/hover` 查询。
    pub fn hover(&mut self, request: &LspDocumentPositionRequest) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request.id,
            method: "textDocument/hover".to_owned(),
            params: json!({
                "textDocument": { "uri": request.text_document_uri },
                "position": { "line": request.line, "character": request.character },
            }),
        })
    }

    /// 执行 `textDocument/implementation` 查询。
    pub fn implementation(&mut self, request: &LspDocumentPositionRequest) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request.id,
            method: "textDocument/implementation".to_owned(),
            params: json!({
                "textDocument": { "uri": request.text_document_uri },
                "position": { "line": request.line, "character": request.character },
            }),
        })
    }

    /// 执行 `textDocument/documentSymbol` 查询。
    pub fn document_symbol(
        &mut self,
        request_id: u64,
        text_document_uri: &str,
    ) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request_id,
            method: "textDocument/documentSymbol".to_owned(),
            params: json!({
                "textDocument": { "uri": text_document_uri },
            }),
        })
    }

    /// 执行 `textDocument/prepareCallHierarchy` 查询。
    pub fn prepare_call_hierarchy(
        &mut self,
        request: &LspDocumentPositionRequest,
    ) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request.id,
            method: "textDocument/prepareCallHierarchy".to_owned(),
            params: json!({
                "textDocument": { "uri": request.text_document_uri },
                "position": { "line": request.line, "character": request.character },
            }),
        })
    }

    /// 执行 `callHierarchy/incomingCalls` 查询。
    pub fn incoming_calls(&mut self, request_id: u64, item: &Value) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request_id,
            method: "callHierarchy/incomingCalls".to_owned(),
            params: json!({ "item": item }),
        })
    }

    /// 执行 `callHierarchy/outgoingCalls` 查询。
    pub fn outgoing_calls(&mut self, request_id: u64, item: &Value) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request_id,
            method: "callHierarchy/outgoingCalls".to_owned(),
            params: json!({ "item": item }),
        })
    }

    /// 执行 `workspace/diagnostic` 查询。
    pub fn workspace_diagnostic(&mut self, request_id: u64) -> AxonResult<Value> {
        self.send_request(&LspClientRequest {
            id: request_id,
            method: "workspace/diagnostic".to_owned(),
            params: json!({ "previousResultIds": [] }),
        })
    }

    /// 关闭底层 LSP 子进程。
    pub fn shutdown(&mut self) {
        let _ = self.stdin.take();
        let _ = self.incoming.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(thread) = self.reader_thread.take() {
            let _ = thread.join();
        }
        if let Some(thread) = self.stderr_thread.take() {
            let _ = thread.join();
        }
    }

    fn write_message(&mut self, message: &str) -> AxonResult<()> {
        let Some(stdin) = self.stdin.as_mut() else {
            return Err(AxonError::tool_invalid_definition("LSP stdin 不可用"));
        };
        let header = format!("Content-Length: {}\r\n\r\n", message.len());
        stdin
            .write_all(header.as_bytes())
            .and_then(|()| stdin.write_all(message.as_bytes()))
            .and_then(|()| stdin.flush())
            .map_err(|_| AxonError::tool_invalid_definition("LSP 消息写入失败"))
    }

    fn read_response_with_timeout(
        &mut self,
        request_id: u64,
        timeout: Duration,
    ) -> AxonResult<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(AxonError::tool_invalid_definition("LSP 响应超时"));
            }
            let message = match self
                .incoming
                .as_ref()
                .ok_or_else(|| AxonError::tool_invalid_definition("LSP stdout 已关闭"))?
                .recv_timeout(remaining)
            {
                Ok(message) => message?,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(AxonError::tool_invalid_definition("LSP 响应超时"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(AxonError::tool_invalid_definition("LSP stdout 已关闭"));
                }
            };
            if message.get("id").and_then(Value::as_u64) == Some(request_id)
                && message.get("method").is_none()
            {
                return Ok(message);
            }
            let _ = self.handle_incoming_message(&message)?;
        }
    }

    fn handle_incoming_message(&mut self, message: &Value) -> AxonResult<Option<PathBuf>> {
        if message.get("method").and_then(Value::as_str) == Some("textDocument/publishDiagnostics")
        {
            if let Some(params) = message.get("params")
                && let Some(uri) = params.get("uri").and_then(Value::as_str)
                && let Ok(path) = canonical_file_path_from_uri(uri)
            {
                self.published_diagnostics
                    .insert(path.clone(), params.clone());
                return Ok(Some(path));
            }
            return Ok(None);
        }
        if message.get("method").is_some() && message.get("id").is_some() {
            self.respond_to_server_request(message)?;
        }
        Ok(None)
    }

    fn respond_to_server_request(&mut self, message: &Value) -> AxonResult<()> {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let result = match method {
            "workspace/configuration" => {
                let count = message
                    .pointer("/params/items")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                Some(Value::Array(vec![Value::Null; count]))
            }
            "workspace/workspaceFolders" => {
                Some(self.root_uri.as_ref().map_or(Value::Null, |uri| {
                    json!([{
                        "uri": uri,
                        "name": workspace_name(uri),
                    }])
                }))
            }
            "client/registerCapability"
            | "client/unregisterCapability"
            | "window/workDoneProgress/create" => Some(Value::Null),
            "workspace/applyEdit" => Some(json!({
                "applied": false,
                "failureReason": "Axon LSP 查询会话不应用 server edit",
            })),
            _ => None,
        };
        let response = result.map_or_else(
            || {
                json!({
                    "jsonrpc": LSP_JSONRPC_VERSION,
                    "id": id,
                    "error": {
                        "code": JSONRPC_METHOD_NOT_FOUND,
                        "message": "Axon LSP client 不支持该 server request",
                    },
                })
            },
            |result| {
                json!({
                    "jsonrpc": LSP_JSONRPC_VERSION,
                    "id": id,
                    "result": result,
                })
            },
        );
        let response_json = serde_json::to_string(&response)
            .map_err(|_| AxonError::tool_invalid_definition("LSP server request 响应序列化失败"))?;
        self.write_message(&response_json)
    }
}

impl Drop for LspStdioClient {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
#[path = "lsp_stdio_client_tests.rs"]
mod tests;
