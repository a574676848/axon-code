//! MCP server 通知事件安全投影。

use serde_json::Value;

/// MCP progress 通知方法名。
pub const MCP_NOTIFICATION_PROGRESS_METHOD: &str = "notifications/progress";
/// MCP cancelled 通知方法名。
pub const MCP_NOTIFICATION_CANCELLED_METHOD: &str = "notifications/cancelled";
/// MCP tools/list_changed 通知方法名。
pub const MCP_NOTIFICATION_TOOLS_LIST_CHANGED_METHOD: &str = "notifications/tools/list_changed";
/// MCP resources/list_changed 通知方法名。
pub const MCP_NOTIFICATION_RESOURCES_LIST_CHANGED_METHOD: &str =
    "notifications/resources/list_changed";
/// MCP resources/updated 通知方法名。
pub const MCP_NOTIFICATION_RESOURCES_UPDATED_METHOD: &str = "notifications/resources/updated";
/// MCP prompts/list_changed 通知方法名。
pub const MCP_NOTIFICATION_PROMPTS_LIST_CHANGED_METHOD: &str = "notifications/prompts/list_changed";
/// MCP logging/message 通知方法名。
pub const MCP_NOTIFICATION_LOGGING_MESSAGE_METHOD: &str = "notifications/message";
/// MCP roots/list 请求方法名。
pub const MCP_SERVER_REQUEST_ROOTS_LIST_METHOD: &str = "roots/list";
/// MCP sampling/createMessage 请求方法名。
pub const MCP_SERVER_REQUEST_SAMPLING_CREATE_MESSAGE_METHOD: &str = "sampling/createMessage";
/// MCP elicitation/create 请求方法名。
pub const MCP_SERVER_REQUEST_ELICITATION_CREATE_METHOD: &str = "elicitation/create";
/// MCP logging/setLevel 请求方法名。
pub const MCP_SERVER_REQUEST_LOGGING_SET_LEVEL_METHOD: &str = "logging/setLevel";
/// 未声明 client capability 时返回给 server 的 JSON-RPC 错误码。
pub const MCP_SERVER_REQUEST_CAPABILITY_DENIED_ERROR_CODE: i64 = -32601;

/// MCP server 通知事件安全投影。
#[derive(Debug, Clone, PartialEq)]
pub struct McpNotificationEvent {
    /// 通知类型。
    pub kind: McpNotificationKind,
    /// progress token 或 request id。
    pub token: Option<String>,
    /// 可展示短消息。
    pub message: Option<String>,
    /// 进度百分比。
    pub percentage: Option<f64>,
    /// 日志级别。
    pub log_level: Option<String>,
    /// 被拒绝的 server request method。
    pub denied_method: Option<String>,
    /// 拒绝响应 JSON-RPC payload。
    pub denial_response_json: Option<String>,
}

/// MCP server 通知类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpNotificationKind {
    /// `notifications/progress`。
    Progress,
    /// `notifications/cancelled`。
    Cancelled,
    /// `notifications/tools/list_changed`。
    ToolsChanged,
    /// `notifications/resources/list_changed`。
    ResourcesChanged,
    /// `notifications/resources/updated`。
    ResourcesUpdated,
    /// `notifications/prompts/list_changed`。
    PromptsChanged,
    /// `notifications/message`。
    LogMessage,
    /// 未声明 client capability 的 server-initiated request。
    ClientRequestDenied,
}

impl McpNotificationKind {
    /// 返回稳定状态码。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Progress => "progress",
            Self::Cancelled => "cancelled",
            Self::ToolsChanged => "tools_changed",
            Self::ResourcesChanged => "resources_changed",
            Self::ResourcesUpdated => "resources_updated",
            Self::PromptsChanged => "prompts_changed",
            Self::LogMessage => "log_message",
            Self::ClientRequestDenied => "client_request_denied",
        }
    }
}

/// MCP server 通知摘要。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McpNotificationSummary {
    /// progress 通知数量。
    pub progress_count: usize,
    /// cancelled 通知数量。
    pub cancelled_count: usize,
    /// tools/list_changed 通知数量。
    pub tools_changed_count: usize,
    /// resources/list_changed 通知数量。
    pub resources_changed_count: usize,
    /// resources/updated 通知数量。
    pub resources_updated_count: usize,
    /// prompts/list_changed 通知数量。
    pub prompts_changed_count: usize,
    /// logging/message 通知数量。
    pub log_message_count: usize,
    /// 被拒绝的 server-initiated request 数量。
    pub denied_request_count: usize,
    /// 已回写给 server 的拒绝响应数量。
    pub denial_response_count: usize,
    /// 最近被拒绝的 server request method。
    pub latest_denied_method: Option<String>,
    /// 最近一条通知。
    pub latest: Option<McpNotificationEvent>,
    /// 是否观察到取消通知。
    pub server_cancelled: bool,
    /// 是否暴露原始通知 payload。
    pub raw_payload_exposed: bool,
}

impl McpNotificationSummary {
    /// 从通知事件构造摘要。
    #[must_use]
    pub fn from_events(events: &[McpNotificationEvent]) -> Self {
        let progress_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::Progress)
            .count();
        let cancelled_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::Cancelled)
            .count();
        let tools_changed_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::ToolsChanged)
            .count();
        let resources_changed_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::ResourcesChanged)
            .count();
        let resources_updated_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::ResourcesUpdated)
            .count();
        let prompts_changed_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::PromptsChanged)
            .count();
        let log_message_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::LogMessage)
            .count();
        let denied_request_count = events
            .iter()
            .filter(|event| event.kind == McpNotificationKind::ClientRequestDenied)
            .count();
        let latest_denied_method = events
            .iter()
            .rev()
            .find_map(|event| event.denied_method.clone());
        let denial_response_count = events
            .iter()
            .filter(|event| event.denial_response_json.is_some())
            .count();
        Self {
            progress_count,
            cancelled_count,
            tools_changed_count,
            resources_changed_count,
            resources_updated_count,
            prompts_changed_count,
            log_message_count,
            denied_request_count,
            denial_response_count,
            latest_denied_method,
            latest: events.last().cloned(),
            server_cancelled: cancelled_count > 0,
            raw_payload_exposed: false,
        }
    }
}

/// 从 JSON-RPC 值中提取 MCP 通知事件。
#[must_use]
pub fn notification_event_from_value(value: &Value) -> Option<McpNotificationEvent> {
    let method = value.get("method").and_then(Value::as_str)?;
    let params = value.get("params").unwrap_or(&Value::Null);
    match method {
        MCP_NOTIFICATION_PROGRESS_METHOD => Some(McpNotificationEvent {
            kind: McpNotificationKind::Progress,
            token: string_field(params, "progressToken"),
            message: string_field(params, "message"),
            percentage: params.get("progress").and_then(Value::as_f64),
            log_level: None,
            denied_method: None,
            denial_response_json: None,
        }),
        MCP_NOTIFICATION_CANCELLED_METHOD => Some(McpNotificationEvent {
            kind: McpNotificationKind::Cancelled,
            token: string_field(params, "requestId"),
            message: string_field(params, "reason"),
            percentage: None,
            log_level: None,
            denied_method: None,
            denial_response_json: None,
        }),
        MCP_NOTIFICATION_TOOLS_LIST_CHANGED_METHOD => {
            Some(change_event(McpNotificationKind::ToolsChanged, params))
        }
        MCP_NOTIFICATION_RESOURCES_LIST_CHANGED_METHOD => {
            Some(change_event(McpNotificationKind::ResourcesChanged, params))
        }
        MCP_NOTIFICATION_RESOURCES_UPDATED_METHOD => Some(resource_updated_event(params)),
        MCP_NOTIFICATION_PROMPTS_LIST_CHANGED_METHOD => {
            Some(change_event(McpNotificationKind::PromptsChanged, params))
        }
        MCP_NOTIFICATION_LOGGING_MESSAGE_METHOD => Some(McpNotificationEvent {
            kind: McpNotificationKind::LogMessage,
            token: None,
            message: logging_message(params),
            percentage: None,
            log_level: string_field(params, "level"),
            denied_method: None,
            denial_response_json: None,
        }),
        _ => server_request_denial_event(value, method),
    }
}

fn change_event(kind: McpNotificationKind, params: &Value) -> McpNotificationEvent {
    McpNotificationEvent {
        kind,
        token: None,
        message: string_field(params, "message"),
        percentage: None,
        log_level: None,
        denied_method: None,
        denial_response_json: None,
    }
}

fn resource_updated_event(params: &Value) -> McpNotificationEvent {
    McpNotificationEvent {
        kind: McpNotificationKind::ResourcesUpdated,
        token: string_field(params, "uri"),
        message: string_field(params, "message"),
        percentage: None,
        log_level: None,
        denied_method: None,
        denial_response_json: None,
    }
}

fn server_request_denial_event(value: &Value, method: &str) -> Option<McpNotificationEvent> {
    if value.get("id").is_none() || method.starts_with("notifications/") {
        return None;
    }
    if !server_request_denied_by_policy(method) {
        return None;
    }
    let denial_response_json =
        serde_json::to_string(&server_request_denial_response(value, method))
            .unwrap_or_else(|_| "{\"jsonrpc\":\"2.0\",\"error\":{\"code\":-32601}}".to_owned());
    Some(McpNotificationEvent {
        kind: McpNotificationKind::ClientRequestDenied,
        token: string_or_number_field(value, "id"),
        message: Some("MCP client capability 未声明，已拒绝 server request".to_owned()),
        percentage: None,
        log_level: None,
        denied_method: Some(method.to_owned()),
        denial_response_json: Some(denial_response_json),
    })
}

fn server_request_denied_by_policy(method: &str) -> bool {
    matches!(
        method,
        MCP_SERVER_REQUEST_ROOTS_LIST_METHOD
            | MCP_SERVER_REQUEST_SAMPLING_CREATE_MESSAGE_METHOD
            | MCP_SERVER_REQUEST_ELICITATION_CREATE_METHOD
            | MCP_SERVER_REQUEST_LOGGING_SET_LEVEL_METHOD
    )
}

fn server_request_denial_response(value: &Value, method: &str) -> Value {
    serde_json::json!({
        "jsonrpc": value.get("jsonrpc").and_then(Value::as_str).unwrap_or("2.0"),
        "id": value.get("id").cloned().unwrap_or(Value::Null),
        "error": {
            "code": MCP_SERVER_REQUEST_CAPABILITY_DENIED_ERROR_CODE,
            "message": format!("MCP client capability for {method} is not advertised"),
        }
    })
}

fn logging_message(params: &Value) -> Option<String> {
    string_field(params, "logger")
        .or_else(|| string_field(params, "message"))
        .or_else(|| {
            params
                .get("data")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
}

/// 从响应文本中提取 MCP 通知事件，支持 JSON 与 SSE。
#[must_use]
pub fn notification_events_from_text(text: &str) -> Vec<McpNotificationEvent> {
    json_values_from_text(text)
        .iter()
        .filter_map(notification_event_from_value)
        .collect()
}

fn json_values_from_text(text: &str) -> Vec<Value> {
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return vec![value];
    }
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("data:"))
        .filter_map(|payload| {
            let payload = payload.trim();
            if payload.is_empty() || payload == "[DONE]" {
                return None;
            }
            serde_json::from_str::<Value>(payload).ok()
        })
        .collect()
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn string_or_number_field(value: &Value, field: &str) -> Option<String> {
    let value = value.get(field)?;
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_i64().map(|number| number.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_progress_and_cancelled_notifications_from_sse() {
        let events = notification_events_from_text(
            r#"data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progressToken":"t1","progress":42.5,"message":"half"}}
data: {"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":"8","reason":"server stopped"}}
data: {"jsonrpc":"2.0","id":8,"result":{}}
"#,
        );
        let summary = McpNotificationSummary::from_events(&events);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, McpNotificationKind::Progress);
        assert_eq!(events[0].token.as_deref(), Some("t1"));
        assert_eq!(events[0].percentage, Some(42.5));
        assert_eq!(events[1].kind, McpNotificationKind::Cancelled);
        assert_eq!(summary.progress_count, 1);
        assert_eq!(summary.cancelled_count, 1);
        assert!(summary.server_cancelled);
        assert!(!summary.raw_payload_exposed);
    }

    #[test]
    fn parses_changed_and_logging_notifications_from_sse() {
        let events = notification_events_from_text(
            r#"data: {"jsonrpc":"2.0","method":"notifications/tools/list_changed","params":{"message":"tools changed"}}
data: {"jsonrpc":"2.0","method":"notifications/resources/list_changed","params":{}}
data: {"jsonrpc":"2.0","method":"notifications/resources/updated","params":{"uri":"axon://team-tools/schema","message":"schema updated"}}
data: {"jsonrpc":"2.0","method":"notifications/prompts/list_changed","params":{}}
data: {"jsonrpc":"2.0","method":"notifications/message","params":{"level":"warning","logger":"mcp.server","data":"hidden raw data"}}
data: {"jsonrpc":"2.0","id":2,"result":{}}
"#,
        );
        let summary = McpNotificationSummary::from_events(&events);

        assert_eq!(events.len(), 5);
        assert_eq!(summary.tools_changed_count, 1);
        assert_eq!(summary.resources_changed_count, 1);
        assert_eq!(summary.resources_updated_count, 1);
        assert_eq!(events[2].kind, McpNotificationKind::ResourcesUpdated);
        assert_eq!(events[2].token.as_deref(), Some("axon://team-tools/schema"));
        assert_eq!(events[2].message.as_deref(), Some("schema updated"));
        assert_eq!(summary.prompts_changed_count, 1);
        assert_eq!(summary.log_message_count, 1);
        assert_eq!(
            summary
                .latest
                .as_ref()
                .and_then(|event| event.log_level.as_deref()),
            Some("warning")
        );
        assert!(!summary.raw_payload_exposed);
    }

    #[test]
    fn denies_server_initiated_requests_without_exposing_raw_payload() {
        let events = notification_events_from_text(
            r#"data: {"jsonrpc":"2.0","id":"root-1","method":"roots/list","params":{"raw":"hidden"}}
data: {"jsonrpc":"2.0","id":2,"result":{}}
"#,
        );
        let summary = McpNotificationSummary::from_events(&events);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, McpNotificationKind::ClientRequestDenied);
        assert_eq!(events[0].denied_method.as_deref(), Some("roots/list"));
        assert_eq!(events[0].token.as_deref(), Some("root-1"));
        assert_eq!(summary.denied_request_count, 1);
        assert_eq!(summary.denial_response_count, 1);
        assert_eq!(summary.latest_denied_method.as_deref(), Some("roots/list"));
        let response = events[0]
            .denial_response_json
            .as_deref()
            .expect("应生成拒绝响应");
        assert!(response.contains(r#""code":-32601"#));
        assert!(!response.contains("hidden"));
        assert!(!summary.raw_payload_exposed);
    }
}
