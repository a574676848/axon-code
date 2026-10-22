use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    thread,
};

use crate::{
    MCP_TRANSPORT_PROTOCOL_VERSION, McpHttpAuthConfig, McpNotificationKind, McpServerHeader,
    McpTransportRuntime,
};

#[path = "transport_test_responses.rs"]
mod transport_test_responses;

use transport_test_responses::{
    resource_templates_paged_response, resources_paged_response, tools_duplicate_cursor_response,
    tools_paged_response,
};

const SESSION_ID: &str = "axon-session-1";

#[test]
fn streamable_http_tools_list_initializes_session_and_parses_sse() {
    let server = spawn_streamable_http_server(ServerMode::ToolsListSse);
    let bearer_env = existing_token_env();
    let bearer_value = std::env::var(&bearer_env).expect("测试 bearer env 应存在");

    let report = McpTransportRuntime::list_streamable_http_tools(
        server.url.clone(),
        &[McpServerHeader {
            name: "X-Team".to_owned(),
            value: "tools".to_owned(),
        }],
        &McpHttpAuthConfig::from_bearer_token_env_var(Some(bearer_env)),
        None,
    )
    .expect("Streamable HTTP tools/list 应可完成");

    assert_eq!(report.tool_schemas.len(), 1);
    assert_eq!(report.next_cursor, None);
    assert_eq!(report.requests_executed, 1);
    assert_eq!(report.tool_schemas[0].name, "query_db");
    assert_eq!(
        report.tool_schemas[0].input_schema,
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "数据库查询" },
                "options": {
                    "type": "object",
                    "properties": {
                        "mode": { "type": "string", "enum": ["fast", "safe"] }
                    }
                },
                "tags": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["query"]
        })
    );
    assert_eq!(
        report.tool_schemas[0].output_schema,
        Some(serde_json::json!({
            "type": "object",
            "properties": { "rows": { "type": "array" } },
            "required": ["rows"]
        }))
    );
    assert_eq!(
        report.tool_schemas[0].annotations.read_only_hint,
        Some(false)
    );
    assert_eq!(
        report.tool_schemas[0].annotations.destructive_hint,
        Some(true)
    );
    assert_eq!(
        report.tool_schemas[0].annotations.open_world_hint,
        Some(true)
    );
    let requests = server.join();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].body.contains(r#""method":"initialize""#));
    assert!(requests[0].body.contains(r#""capabilities":{}"#));
    assert!(!requests[0].body.contains(r#""elicitation""#));
    assert!(!requests[0].body.contains(r#""roots""#));
    assert!(!requests[0].body.contains(r#""sampling""#));
    assert!(!requests[0].body.contains(r#""logging""#));
    assert!(
        requests[1]
            .body
            .contains(r#""method":"notifications/initialized""#)
    );
    assert!(requests[2].body.contains(r#""method":"tools/list""#));
    assert_header(&requests[2].raw, "Mcp-Session-Id", SESSION_ID);
    assert_header(
        &requests[2].raw,
        "Authorization",
        &format!("Bearer {bearer_value}"),
    );
    assert_header(
        &requests[2].raw,
        "MCP-Protocol-Version",
        MCP_TRANSPORT_PROTOCOL_VERSION,
    );
    assert_eq!(report.notification_summary.progress_count, 0);
    assert!(!report.notification_summary.raw_payload_exposed);
}

#[test]
fn streamable_http_tools_list_projects_progress_notification() {
    let server = spawn_streamable_http_server(ServerMode::ToolsListSseWithProgress);

    let report = McpTransportRuntime::list_streamable_http_tools(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        None,
    )
    .expect("Streamable HTTP tools/list 应可投影 progress 通知");

    assert_eq!(report.tool_schemas.len(), 1);
    assert_eq!(report.notification_summary.progress_count, 1);
    assert_eq!(report.notification_summary.cancelled_count, 0);
    assert!(!report.notification_summary.server_cancelled);
    assert!(!report.notification_summary.raw_payload_exposed);
    assert_eq!(
        report
            .notification_summary
            .latest
            .as_ref()
            .and_then(|event| event.token.as_deref()),
        Some("tools-list")
    );
    let requests = server.join();
    assert_eq!(requests.len(), 3);
}

#[test]
fn streamable_http_tools_list_projects_changed_and_log_notifications() {
    let server = spawn_streamable_http_server(ServerMode::ToolsListSseWithChangedAndLog);

    let report = McpTransportRuntime::list_streamable_http_tools(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        None,
    )
    .expect("Streamable HTTP tools/list 应可投影 changed 与 logging 通知");

    assert_eq!(report.tool_schemas.len(), 1);
    assert_eq!(report.notification_summary.tools_changed_count, 1);
    assert_eq!(report.notification_summary.resources_changed_count, 0);
    assert_eq!(report.notification_summary.prompts_changed_count, 0);
    assert_eq!(report.notification_summary.log_message_count, 1);
    assert_eq!(report.notification_summary.denied_request_count, 1);
    assert_eq!(
        report.notification_summary.latest_denied_method.as_deref(),
        Some("roots/list")
    );
    assert_eq!(
        report
            .notification_summary
            .latest
            .as_ref()
            .map(|event| event.kind),
        Some(McpNotificationKind::ClientRequestDenied)
    );
    assert!(!report.notification_summary.raw_payload_exposed);
    let requests = server.join();
    assert_eq!(requests.len(), 4);
    assert!(requests[3].body.contains(r#""id":"roots-1""#));
    assert!(requests[3].body.contains(r#""code":-32601"#));
    assert!(!requests[3].body.contains("hidden"));
    assert_header(&requests[3].raw, "Mcp-Session-Id", SESSION_ID);
}

#[test]
fn streamable_http_tools_list_follows_next_cursor() {
    let server = spawn_streamable_http_server(ServerMode::ToolsListPagedJson);

    let report = McpTransportRuntime::list_streamable_http_tools_with_pagination(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
    )
    .expect("Streamable HTTP tools/list 应跟随 nextCursor");

    assert_eq!(report.tool_schemas.len(), 2);
    assert_eq!(report.requests_executed, 2);
    assert_eq!(report.tool_schemas[0].name, "query_db");
    assert_eq!(report.tool_schemas[1].name, "explain_db");
    let requests = server.join();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].body.contains(r#""method":"tools/list""#));
    assert!(requests[3].body.contains(r#""cursor":"tools-page-2""#));
}

#[test]
fn streamable_http_tools_list_rejects_duplicate_cursor() {
    let server = spawn_streamable_http_server(ServerMode::ToolsListDuplicateCursor);

    let error = McpTransportRuntime::list_streamable_http_tools_with_pagination(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
    )
    .expect_err("重复 cursor 必须被拒绝");

    assert!(format!("{error:?}").contains("重复 cursor"));
    let requests = server.join();
    assert_eq!(requests.len(), 4);
}

#[test]
fn streamable_http_tool_call_uses_initialized_session() {
    let server = spawn_streamable_http_server(ServerMode::ToolCallJson);

    let report = McpTransportRuntime::call_streamable_http_tool(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        "query_db",
        r#"{"query":"ORBIT-9"}"#,
        None,
    )
    .expect("Streamable HTTP tools/call 应可完成");

    assert!(report.result_text.contains("ORBIT-9"));
    assert!(!report.result_projection.is_error);
    assert_eq!(report.result_projection.content_types, ["text"]);
    assert!(report.result_projection.structured_content_declared);
    assert_eq!(report.result_projection.structured_content_keys, ["rows"]);
    assert!(!report.result_projection.raw_payload_exposed);
    let requests = server.join();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].body.contains(r#""method":"tools/call""#));
    assert!(requests[2].body.contains("ORBIT-9"));
    assert_header(&requests[2].raw, "Mcp-Session-Id", SESSION_ID);
}

#[test]
fn streamable_http_tool_call_projects_cancelled_notification() {
    let server = spawn_streamable_http_server(ServerMode::ToolCallSseWithCancelled);

    let report = McpTransportRuntime::call_streamable_http_tool(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        "query_db",
        r#"{"query":"ORBIT-9"}"#,
        Some(std::time::Duration::from_secs(1)),
    )
    .expect("Streamable HTTP tools/call 应可投影 cancelled 通知");

    assert!(report.result_text.contains("ORBIT-9"));
    assert_eq!(report.notification_summary.progress_count, 0);
    assert_eq!(report.notification_summary.cancelled_count, 1);
    assert!(report.notification_summary.server_cancelled);
    assert!(!report.notification_summary.raw_payload_exposed);
    assert_eq!(
        report
            .notification_summary
            .latest
            .as_ref()
            .and_then(|event| event.message.as_deref()),
        Some("server cancelled request")
    );
    let requests = server.join();
    assert_eq!(requests.len(), 3);
}

#[test]
fn streamable_http_resources_list_uses_initialized_session() {
    let server = spawn_streamable_http_server(ServerMode::ResourcesListJson);

    let report = McpTransportRuntime::list_streamable_http_resources(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        None,
    )
    .expect("Streamable HTTP resources/list 应可完成");

    assert_eq!(report.resources.len(), 1);
    assert_eq!(report.next_cursor, None);
    assert_eq!(report.requests_executed, 1);
    assert_eq!(report.resources[0].uri, "docs://guide");
    assert_eq!(
        report.resources[0].mime_type.as_deref(),
        Some("text/markdown")
    );
    let requests = server.join();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].body.contains(r#""method":"resources/list""#));
    assert_header(&requests[2].raw, "Mcp-Session-Id", SESSION_ID);
}

#[test]
fn streamable_http_resources_list_follows_next_cursor() {
    let server = spawn_streamable_http_server(ServerMode::ResourcesListPagedJson);

    let report = McpTransportRuntime::list_streamable_http_resources_with_pagination(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
    )
    .expect("Streamable HTTP resources/list 应跟随 nextCursor");

    assert_eq!(report.resources.len(), 2);
    assert_eq!(report.requests_executed, 2);
    assert_eq!(report.resources[0].uri, "docs://guide");
    assert_eq!(report.resources[1].uri, "docs://api");
    let requests = server.join();
    assert_eq!(requests.len(), 4);
    assert!(requests[2].body.contains(r#""method":"resources/list""#));
    assert!(requests[3].body.contains(r#""cursor":"page-2""#));
}

#[test]
fn streamable_http_resource_templates_list_parses_sse() {
    let server = spawn_streamable_http_server(ServerMode::ResourceTemplatesListSse);

    let report = McpTransportRuntime::list_streamable_http_resource_templates(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        None,
    )
    .expect("Streamable HTTP resources/templates/list 应可完成");

    assert_eq!(report.templates.len(), 1);
    assert_eq!(report.next_cursor, None);
    assert_eq!(report.requests_executed, 1);
    assert_eq!(report.templates[0].uri_template, "docs://{name}");
    let requests = server.join();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[2]
            .body
            .contains(r#""method":"resources/templates/list""#)
    );
    assert_header(&requests[2].raw, "Mcp-Session-Id", SESSION_ID);
}

#[test]
fn streamable_http_resource_templates_list_follows_next_cursor() {
    let server = spawn_streamable_http_server(ServerMode::ResourceTemplatesListPagedJson);

    let report = McpTransportRuntime::list_streamable_http_resource_templates_with_pagination(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
    )
    .expect("Streamable HTTP resources/templates/list 应跟随 nextCursor");

    assert_eq!(report.templates.len(), 2);
    assert_eq!(report.requests_executed, 2);
    assert_eq!(report.templates[0].uri_template, "docs://{name}");
    assert_eq!(report.templates[1].uri_template, "docs://api/{name}");
    let requests = server.join();
    assert_eq!(requests.len(), 4);
    assert!(
        requests[2]
            .body
            .contains(r#""method":"resources/templates/list""#)
    );
    assert!(requests[3].body.contains(r#""cursor":"templates-page-2""#));
}

#[test]
fn streamable_http_resource_read_returns_contents() {
    let server = spawn_streamable_http_server(ServerMode::ResourceReadJson);

    let report = McpTransportRuntime::read_streamable_http_resource(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        "docs://guide",
    )
    .expect("Streamable HTTP resources/read 应可完成");

    assert_eq!(report.contents.len(), 1);
    assert_eq!(report.contents[0].uri, "docs://guide");
    assert!(
        report.contents[0]
            .text
            .as_deref()
            .is_some_and(|text| text.contains("Axon Guide"))
    );
    let requests = server.join();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].body.contains(r#""method":"resources/read""#));
    assert!(requests[2].body.contains("docs://guide"));
    assert_header(&requests[2].raw, "Mcp-Session-Id", SESSION_ID);
}

#[test]
fn streamable_http_prompts_list_accepts_cursor_and_projects_next_cursor() {
    let server = spawn_streamable_http_server(ServerMode::PromptsListJson);

    let report = McpTransportRuntime::list_streamable_http_prompts(
        server.url.clone(),
        &[],
        &McpHttpAuthConfig::disabled(),
        Some("prompt-page-2"),
    )
    .expect("Streamable HTTP prompts/list 应支持 cursor");

    assert_eq!(report.prompts.len(), 1);
    assert_eq!(report.prompts[0].name, "incident_summary");
    assert_eq!(report.next_cursor.as_deref(), Some("prompt-page-3"));
    assert_eq!(report.requests_executed, 1);
    let requests = server.join();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].body.contains(r#""method":"prompts/list""#));
    assert!(requests[2].body.contains(r#""cursor":"prompt-page-2""#));
    assert_header(&requests[2].raw, "Mcp-Session-Id", SESSION_ID);
}

#[derive(Clone, Copy)]
enum ServerMode {
    ToolsListSse,
    ToolsListSseWithProgress,
    ToolsListSseWithChangedAndLog,
    ToolsListPagedJson,
    ToolsListDuplicateCursor,
    ToolCallJson,
    ToolCallSseWithCancelled,
    ResourcesListJson,
    ResourcesListPagedJson,
    ResourceTemplatesListSse,
    ResourceTemplatesListPagedJson,
    ResourceReadJson,
    PromptsListJson,
}

struct TestServer {
    url: String,
    receiver: mpsc::Receiver<Vec<CapturedRequest>>,
}

impl TestServer {
    fn join(self) -> Vec<CapturedRequest> {
        self.receiver
            .recv()
            .expect("测试 HTTP server 应返回请求记录")
    }
}

struct CapturedRequest {
    raw: String,
    body: String,
}

fn spawn_streamable_http_server(mode: ServerMode) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("测试 HTTP server 应可绑定端口");
    let url = format!(
        "http://{}",
        listener.local_addr().expect("本地地址应可读取")
    );
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..request_count(mode) {
            let (stream, _) = listener.accept().expect("测试请求应可接收");
            let request = read_request(stream, index, mode);
            requests.push(request);
        }
        let _ = sender.send(requests);
    });
    TestServer { url, receiver }
}

fn existing_token_env() -> String {
    ["USERNAME", "USERPROFILE", "PATH"]
        .into_iter()
        .find(|name| std::env::var(name).is_ok_and(|value| !value.trim().is_empty()))
        .expect("测试进程应至少包含一个基础环境变量")
        .to_owned()
}

fn request_count(mode: ServerMode) -> usize {
    match mode {
        ServerMode::ToolsListPagedJson | ServerMode::ToolsListDuplicateCursor => 4,
        ServerMode::ResourcesListPagedJson => 4,
        ServerMode::ResourceTemplatesListPagedJson => 4,
        ServerMode::ToolsListSseWithChangedAndLog => 4,
        _ => 3,
    }
}

fn read_request(mut stream: TcpStream, index: usize, mode: ServerMode) -> CapturedRequest {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).expect("请求应可读取");
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(request) = captured_request(&buffer) {
            write_response(&mut stream, index, mode);
            return request;
        }
    }
    panic!("请求未完整读取");
}

fn captured_request(buffer: &[u8]) -> Option<CapturedRequest> {
    let raw = String::from_utf8_lossy(buffer).to_string();
    let header_end = find_header_end(buffer)?;
    let content_length = raw
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value)
        })
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or_default();
    let body_start = header_end + 4;
    if buffer.len() < body_start + content_length {
        return None;
    }
    let body =
        String::from_utf8_lossy(&buffer[body_start..body_start + content_length]).to_string();
    Some(CapturedRequest { raw, body })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}

fn write_response(stream: &mut TcpStream, index: usize, mode: ServerMode) {
    let (content_type, body) = match index {
        0 => (
            "application/json",
            format!(
                r#"{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"{}","capabilities":{{}},"serverInfo":{{"name":"test","version":"1.0.0"}}}}}}"#,
                MCP_TRANSPORT_PROTOCOL_VERSION
            ),
        ),
        1 => ("application/json", String::new()),
        _ => match mode {
            ServerMode::ToolsListSse => (
                "text/event-stream",
                r#"data: {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query_db","description":"query db","inputSchema":{"type":"object","properties":{"query":{"type":"string","description":"数据库查询"},"options":{"type":"object","properties":{"mode":{"type":"string","enum":["fast","safe"]}}},"tags":{"type":"array","items":{"type":"string"}}},"required":["query"]},"outputSchema":{"type":"object","properties":{"rows":{"type":"array"}},"required":["rows"]},"annotations":{"readOnlyHint":false,"destructiveHint":true,"openWorldHint":true}}]}}

"#
                .to_owned(),
            ),
            ServerMode::ToolsListSseWithProgress => (
                "text/event-stream",
                r#"data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"tools-list","progress":50.0,"message":"listing tools"}}
data: {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query_db","description":"query db","inputSchema":{"type":"object","required":["query"]}}]}}

"#
                .to_owned(),
            ),
            ServerMode::ToolsListSseWithChangedAndLog => (
                "text/event-stream",
                r#"data: {"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{"message":"tools changed"}}
data: {"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","logger":"fixture","data":"raw server log"}}
data: {"jsonrpc":"2.0","id":"roots-1","method":"roots/list","params":{"raw":"hidden"}}
data: {"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"query_db","description":"query db","inputSchema":{"type":"object","required":["query"]}}]}}

"#
                .to_owned(),
            ),
            ServerMode::ToolsListPagedJson => tools_paged_response(index),
            ServerMode::ToolsListDuplicateCursor => tools_duplicate_cursor_response(),
            ServerMode::ToolCallJson => (
                "application/json",
                r#"{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"MCP_RESULT ORBIT-9"}],"structuredContent":{"rows":[{"id":"ORBIT-9"}]},"isError":false}}"#
                    .to_owned(),
            ),
            ServerMode::ToolCallSseWithCancelled => (
                "text/event-stream",
                r#"data: {"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"3","reason":"server cancelled request"}}
data: {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"MCP_RESULT ORBIT-9"}]}}

"#
                .to_owned(),
            ),
            ServerMode::ResourcesListJson => (
                "application/json",
                r#"{"jsonrpc":"2.0","id":4,"result":{"resources":[{"uri":"docs://guide","name":"Guide","description":"Guide docs","mimeType":"text/markdown"}]}}"#
                    .to_owned(),
            ),
            ServerMode::ResourcesListPagedJson => resources_paged_response(index),
            ServerMode::ResourceTemplatesListSse => (
                "text/event-stream",
                r#"data: {"jsonrpc":"2.0","id":5,"result":{"resourceTemplates":[{"uriTemplate":"docs://{name}","name":"Docs","mimeType":"text/markdown"}]}}

"#
                .to_owned(),
            ),
            ServerMode::ResourceTemplatesListPagedJson => resource_templates_paged_response(index),
            ServerMode::ResourceReadJson => (
                "application/json",
                r##"{"jsonrpc":"2.0","id":6,"result":{"contents":[{"uri":"docs://guide","mimeType":"text/markdown","text":"# Axon Guide"}]}}"##
                    .to_owned(),
            ),
            ServerMode::PromptsListJson => ("application/json", r#"{"jsonrpc":"2.0","id":7,"result":{"prompts":[{"name":"incident_summary","title":"Incident Summary","description":"incident prompt","arguments":[{"name":"incident","required":true}]}],"nextCursor":"prompt-page-3"}}"#.to_owned()),
        },
    };
    let session_header = if index == 0 {
        format!("Mcp-Session-Id: {SESSION_ID}\r\n")
    } else {
        String::new()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n{session_header}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("测试响应应可写入");
}

fn assert_header(raw: &str, expected_name: &str, expected_value: &str) {
    let found = raw.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.eq_ignore_ascii_case(expected_name) && value.trim() == expected_value
        })
    });
    assert!(
        found,
        "missing header {expected_name}: {expected_value}\n{raw}"
    );
}
