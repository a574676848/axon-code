//! Responses WebSocket 请求链状态与增量 input 计算。

use axon_agent_runtime::ProviderStreamingTransportFailureKind;
use serde_json::Value;

use crate::provider_transport::provider_response_failure::{
    ProviderResponseFailure, classify_terminal_event,
};

const RESPONSE_CREATE_EVENT_TYPE: &str = "response.create";
const RESPONSE_COMPLETED_EVENT_TYPE: &str = "response.completed";
const PREVIOUS_RESPONSE_NOT_FOUND_CODE: &str = "previous_response_not_found";

#[derive(Debug, Clone)]
pub(super) struct WebSocketResponseChain {
    request: Value,
    response_id: String,
    output: Vec<Value>,
    bootstrap: bool,
}

#[derive(Debug)]
pub(super) struct PreparedWebSocketRequest {
    pub(super) wire_text: String,
    pub(super) full_request: Option<Value>,
}

#[derive(Debug, Default)]
pub(super) struct WebSocketResponseState {
    pub(super) completed: Option<CompletedWebSocketResponse>,
    pub(super) failure: Option<ProviderResponseFailure>,
    pub(super) previous_response_not_found: bool,
}

#[derive(Debug, Clone)]
pub(super) struct CompletedWebSocketResponse {
    response_id: String,
    output: Vec<Value>,
}

impl WebSocketResponseChain {
    pub(super) fn bootstrap(request: Value, response: CompletedWebSocketResponse) -> Self {
        Self::new(request, response, true)
    }

    pub(super) fn completed(request: Value, response: CompletedWebSocketResponse) -> Self {
        Self::new(request, response, false)
    }

    fn new(request: Value, response: CompletedWebSocketResponse, bootstrap: bool) -> Self {
        Self {
            request,
            response_id: response.response_id,
            output: response.output,
            bootstrap,
        }
    }
}

pub(super) fn prepare_warmup_request(body_text: &str) -> Option<PreparedWebSocketRequest> {
    let mut request = normalized_response_request(body_text)?;
    request["input"] = Value::Array(Vec::new());
    let mut wire = request.clone();
    wire["type"] = Value::String(RESPONSE_CREATE_EVENT_TYPE.to_owned());
    wire["generate"] = Value::Bool(false);
    Some(PreparedWebSocketRequest {
        wire_text: wire.to_string(),
        full_request: Some(request),
    })
}

pub(super) fn prepare_request(
    body_text: &str,
    chain: Option<&WebSocketResponseChain>,
    server_conversation_state_allowed: bool,
) -> PreparedWebSocketRequest {
    let Some(request) = normalized_response_request(body_text) else {
        return PreparedWebSocketRequest {
            wire_text: body_text.to_owned(),
            full_request: None,
        };
    };
    let incremental = server_conversation_state_allowed
        .then(|| chain.and_then(|chain| incremental_input(chain, &request)))
        .flatten();
    let mut wire = request.clone();
    wire["type"] = Value::String(RESPONSE_CREATE_EVENT_TYPE.to_owned());
    if let (Some(chain), Some(input)) = (chain, incremental) {
        wire["previous_response_id"] = Value::String(chain.response_id.clone());
        wire["input"] = Value::Array(input);
    }
    PreparedWebSocketRequest {
        wire_text: wire.to_string(),
        full_request: Some(request),
    }
}

pub(super) fn observe_response_event(state: &mut WebSocketResponseState, text: &str) {
    let Ok(event) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if let Some(mut failure) = classify_terminal_event(&event) {
        state.previous_response_not_found = event.pointer("/error/code").and_then(Value::as_str)
            == Some(PREVIOUS_RESPONSE_NOT_FOUND_CODE);
        if state.previous_response_not_found {
            failure.kind = ProviderStreamingTransportFailureKind::Incomplete;
        }
        state.failure = Some(failure);
        return;
    }
    if event.get("type").and_then(Value::as_str) == Some(RESPONSE_COMPLETED_EVENT_TYPE) {
        state.completed = completed_response(&event);
    }
}

fn normalized_response_request(body_text: &str) -> Option<Value> {
    let Value::Object(mut body) = serde_json::from_str::<Value>(body_text).ok()? else {
        return None;
    };
    if !body.contains_key("input") || body.contains_key("messages") {
        return None;
    }
    for field in [
        "type",
        "stream",
        "background",
        "generate",
        "previous_response_id",
    ] {
        body.remove(field);
    }
    Some(Value::Object(body))
}

fn incremental_input(chain: &WebSocketResponseChain, request: &Value) -> Option<Vec<Value>> {
    let current_input = request.get("input")?.as_array()?;
    let previous_input = chain.request.get("input")?.as_array()?;
    if chain.bootstrap && previous_input.is_empty() {
        return Some(current_input.clone());
    }
    if request_properties(request) != request_properties(&chain.request) {
        return None;
    }
    let tail = current_input.strip_prefix(previous_input.as_slice())?;
    consume_response_output(tail, &chain.output).map(<[Value]>::to_vec)
}

fn request_properties(request: &Value) -> Value {
    let mut properties = request.clone();
    if let Some(object) = properties.as_object_mut() {
        object.remove("input");
        object.remove("client_metadata");
        // Deferred discovery 只扩展可调用工具及其派生缓存键，不改变 previous_response_id 上下文。
        object.remove("tools");
        object.remove("prompt_cache_key");
    }
    properties
}

fn consume_response_output<'a>(mut input: &'a [Value], output: &[Value]) -> Option<&'a [Value]> {
    for item in output {
        if item.get("type").and_then(Value::as_str) == Some("reasoning") {
            continue;
        }
        let candidate = input.first()?;
        if canonical_response_item(candidate) != canonical_response_item(item) {
            return None;
        }
        input = &input[1..];
    }
    Some(input)
}

fn canonical_response_item(value: &Value) -> Value {
    let Value::Object(object) = value else {
        return value.clone();
    };
    let mut canonical = object.clone();
    for field in ["id", "status", "metadata"] {
        canonical.remove(field);
    }
    Value::Object(canonical)
}

fn completed_response(event: &Value) -> Option<CompletedWebSocketResponse> {
    let response = event.get("response")?;
    let response_id = response.get("id")?.as_str()?.trim();
    if response_id.is_empty() {
        return None;
    }
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Some(CompletedWebSocketResponse {
        response_id: response_id.to_owned(),
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INITIAL_PROMPT_CACHE_KEY: &str = "prompt-cache-initial";
    const ACTIVATED_PROMPT_CACHE_KEY: &str = "prompt-cache-activated";
    const BUILTIN_PROVIDER_NAME: &str = "builtin_search";
    const MCP_LIST_REPOS_PROVIDER_NAME: &str = "mcp__gitnexus__list_repos";

    fn request(input: Value) -> String {
        request_with_tools(input, serde_json::json!([]), INITIAL_PROMPT_CACHE_KEY)
    }

    fn request_with_tools(input: Value, tools: Value, prompt_cache_key: &str) -> String {
        serde_json::json!({
            "model": "gpt-5.6",
            "input": input,
            "stream": true,
            "store": false,
            "tools": tools,
            "prompt_cache_key": prompt_cache_key,
        })
        .to_string()
    }

    #[test]
    fn warmup_uses_top_level_generate_false_without_transport_fields() {
        let prepared = prepare_warmup_request(&request(serde_json::json!([{"type":"message"}])))
            .expect("Responses request should support warmup");
        let wire: Value = serde_json::from_str(&prepared.wire_text).unwrap();

        assert_eq!(wire["type"], RESPONSE_CREATE_EVENT_TYPE);
        assert_eq!(wire["generate"], false);
        assert_eq!(wire["input"], serde_json::json!([]));
        assert!(wire.get("stream").is_none());
        assert!(wire.get("response").is_none());
    }

    #[test]
    fn completed_chain_sends_only_new_tool_output() {
        let first = prepare_request(
            &request(serde_json::json!([{"type":"message","role":"user"}])),
            None,
            true,
        );
        let chain = WebSocketResponseChain::completed(
            first.full_request.unwrap(),
            CompletedWebSocketResponse {
                response_id: "resp-1".to_owned(),
                output: vec![serde_json::json!({
                    "type":"function_call",
                    "id":"fc-1",
                    "call_id":"call-1",
                    "name":"shell",
                    "arguments":"{}",
                    "status":"completed"
                })],
            },
        );
        let current = request(serde_json::json!([
            {"type":"message","role":"user"},
            {"type":"function_call","call_id":"call-1","name":"shell","arguments":"{}"},
            {"type":"function_call_output","call_id":"call-1","output":"ok"}
        ]));
        let prepared = prepare_request(&current, Some(&chain), true);
        let wire: Value = serde_json::from_str(&prepared.wire_text).unwrap();

        assert_eq!(wire["previous_response_id"], "resp-1");
        assert_eq!(
            wire["input"],
            serde_json::json!([{"type":"function_call_output","call_id":"call-1","output":"ok"}])
        );
    }

    #[test]
    fn tool_change_keeps_incremental_response_chain() {
        let first = prepare_request(
            &request_with_tools(
                serde_json::json!([{"type":"message","role":"user"}]),
                serde_json::json!([{"type":"function","name":BUILTIN_PROVIDER_NAME}]),
                INITIAL_PROMPT_CACHE_KEY,
            ),
            None,
            true,
        );
        let chain = WebSocketResponseChain::completed(
            first.full_request.unwrap(),
            CompletedWebSocketResponse {
                response_id: "resp-1".to_owned(),
                output: vec![serde_json::json!({
                    "type":"function_call",
                    "call_id":"call-1",
                    "name":BUILTIN_PROVIDER_NAME,
                    "arguments":"{}"
                })],
            },
        );
        let current = request_with_tools(
            serde_json::json!([
                {"type":"message","role":"user"},
                {
                    "type":"function_call",
                    "call_id":"call-1",
                    "name":BUILTIN_PROVIDER_NAME,
                    "arguments":"{}"
                },
                {"type":"function_call_output","call_id":"call-1","output":"activated"}
            ]),
            serde_json::json!([
                {"type":"function","name":BUILTIN_PROVIDER_NAME},
                {"type":"function","name":MCP_LIST_REPOS_PROVIDER_NAME}
            ]),
            ACTIVATED_PROMPT_CACHE_KEY,
        );
        let prepared = prepare_request(&current, Some(&chain), true);
        let wire: Value = serde_json::from_str(&prepared.wire_text).unwrap();

        assert_eq!(wire["previous_response_id"], "resp-1");
        assert_eq!(
            wire["input"],
            serde_json::json!([{
                "type":"function_call_output",
                "call_id":"call-1",
                "output":"activated"
            }])
        );
        assert_eq!(wire["tools"].as_array().map(Vec::len), Some(2));
    }

    #[test]
    fn property_change_starts_a_full_request_chain() {
        let first = prepare_request(&request(serde_json::json!([])), None, true);
        let chain = WebSocketResponseChain::completed(
            first.full_request.unwrap(),
            CompletedWebSocketResponse {
                response_id: "resp-1".to_owned(),
                output: Vec::new(),
            },
        );
        let changed = serde_json::json!({
            "model":"gpt-5.6",
            "instructions":"changed",
            "input":[{"type":"message"}],
            "store":false,
            "tools":[]
        })
        .to_string();
        let prepared = prepare_request(&changed, Some(&chain), true);
        let wire: Value = serde_json::from_str(&prepared.wire_text).unwrap();

        assert!(wire.get("previous_response_id").is_none());
        assert_eq!(wire["input"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn disabled_server_state_keeps_full_input_without_previous_response_id() {
        let first = prepare_request(
            &request(serde_json::json!([{"type":"message","role":"user"}])),
            None,
            true,
        );
        let chain = WebSocketResponseChain::completed(
            first.full_request.unwrap(),
            CompletedWebSocketResponse {
                response_id: "resp-1".to_owned(),
                output: Vec::new(),
            },
        );
        let current = request(serde_json::json!([
            {"type":"message","role":"user","content":"first"},
            {"type":"message","role":"user","content":"second"}
        ]));
        let prepared = prepare_request(&current, Some(&chain), false);
        let wire: Value = serde_json::from_str(&prepared.wire_text).unwrap();

        assert!(wire.get("previous_response_id").is_none());
        assert_eq!(wire["input"].as_array().map(Vec::len), Some(2));
    }
}
