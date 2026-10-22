//! Feishu 消息查询 helper。
//!
//! 本模块只用于入站 reply 引用补全：按 message_id 查询父消息，并提取 Agent 可见文本。

use axon_domain::{AxonError, AxonResult};

use crate::{
    REMOTE_FEISHU_API_BASE_URL, RemoteFeishuChatType, RemoteFeishuHttpRequestPlan,
    RemoteFeishuHttpResponse, remote_feishu_gateway_sdk_payload::visible_text_from_content,
};

const MESSAGE_LOOKUP_DISPATCH_ID: &str = "feishu-message-lookup";
const MESSAGE_LOOKUP_PATH: &str = "/open-apis/im/v1/messages";
const CHAT_LOOKUP_DISPATCH_ID: &str = "feishu-chat-lookup";
const CHAT_LOOKUP_PATH: &str = "/open-apis/im/v1/chats";
const AUTHORIZATION_HEADER: &str = "Authorization";
const BEARER_PREFIX: &str = "Bearer ";

/// Feishu 消息查询 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoteFeishuMessageLookupRuntime;

/// Feishu message get 响应里的路由信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuMessageLookupInfo {
    /// 发送者 ID。机器人消息在 Feishu 响应中对应 app_id。
    pub sender_id: Option<String>,
    /// 消息所在 chat。
    pub chat_id: Option<String>,
}

/// Feishu reply-text 查询降级阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFeishuReplyLookupFailureStage {
    /// HTTP transport 调用失败。
    Http,
    /// HTTP 状态码非 2xx。
    Status,
    /// Feishu 业务 code 非零。
    FeishuCode,
    /// 响应 JSON 或必要字段无效。
    Json,
    /// 消息正文结构无效。
    Content,
}

/// Feishu reply-text 查询降级信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFeishuReplyLookupFailure {
    /// 失败阶段。
    pub stage: RemoteFeishuReplyLookupFailureStage,
    /// 可选 HTTP 状态码。
    pub http_status: Option<u16>,
    /// 可选 Feishu 业务 code。
    pub feishu_code: Option<i64>,
}

/// Feishu reply-text 专用查询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFeishuReplyLookupOutcome {
    /// 查询到 Agent 可见文本。
    VisibleText(String),
    /// 查询成功，但目标消息没有可见文本。
    NoVisibleText,
    /// 查询降级，当前入站消息应继续处理。
    Degraded(RemoteFeishuReplyLookupFailure),
}

impl RemoteFeishuReplyLookupFailure {
    const fn new(
        stage: RemoteFeishuReplyLookupFailureStage,
        http_status: Option<u16>,
        feishu_code: Option<i64>,
    ) -> Self {
        Self {
            stage,
            http_status,
            feishu_code,
        }
    }
}

impl RemoteFeishuMessageLookupRuntime {
    /// 规划 message get 请求。
    pub fn plan_get_message_request(
        api_base_url: Option<&str>,
        tenant_access_token: &str,
        message_id: &str,
    ) -> AxonResult<RemoteFeishuHttpRequestPlan> {
        if tenant_access_token.trim().is_empty() || message_id.trim().is_empty() {
            return Err(AxonError::runtime_failure("Feishu 消息查询输入无效"));
        }
        let base = normalized_base_url(api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL))?;
        let mut url = url::Url::parse(&base)
            .map_err(|_| AxonError::runtime_failure("Feishu 消息查询 URL 无效"))?;
        url.set_path(MESSAGE_LOOKUP_PATH);
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .map_err(|_| AxonError::runtime_failure("Feishu 消息查询 URL 无法追加路径"))?
            .push(message_id);
        Ok(RemoteFeishuHttpRequestPlan {
            dispatch_id: MESSAGE_LOOKUP_DISPATCH_ID.to_owned(),
            url: url.into(),
            method: "GET".to_owned(),
            headers: vec![(
                AUTHORIZATION_HEADER.to_owned(),
                format!("{BEARER_PREFIX}{tenant_access_token}"),
            )],
            body_json: String::new(),
        })
    }

    /// 从 message get 响应中提取可见文本。
    pub fn parse_message_text(response: &RemoteFeishuHttpResponse) -> AxonResult<Option<String>> {
        let Some(item) = first_success_item(response)? else {
            return Ok(None);
        };
        let message_type = item
            .get("msg_type")
            .or_else(|| item.get("message_type"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let content = item
            .get("body")
            .and_then(|body| body.get("content"))
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        Ok(visible_text_from_content(message_type, content))
    }

    /// 从 message get 响应中提取 reply-text 专用 typed outcome。
    #[must_use]
    pub fn parse_reply_lookup_outcome(
        response: &RemoteFeishuHttpResponse,
    ) -> RemoteFeishuReplyLookupOutcome {
        if !(200..300).contains(&response.status) {
            return RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
                RemoteFeishuReplyLookupFailureStage::Status,
                Some(response.status),
                None,
            ));
        }
        let value = match serde_json::from_str::<serde_json::Value>(&response.body) {
            Ok(value) => value,
            Err(_) => {
                return degraded_reply_lookup(
                    RemoteFeishuReplyLookupFailureStage::Json,
                    response.status,
                    None,
                );
            }
        };
        let items = match validated_reply_items(&value, response.status) {
            Ok(items) => items,
            Err(outcome) => return outcome,
        };
        let Some(item) = items.first() else {
            return RemoteFeishuReplyLookupOutcome::NoVisibleText;
        };
        reply_text_outcome(item, response.status)
    }

    /// 查询并提取父消息 reply-text typed outcome。
    pub fn fetch_reply_lookup_with_http<F>(
        api_base_url: Option<&str>,
        tenant_access_token: &str,
        message_id: &str,
        mut http: F,
    ) -> AxonResult<RemoteFeishuReplyLookupOutcome>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let request =
            Self::plan_get_message_request(api_base_url, tenant_access_token, message_id)?;
        let response = match http(&request) {
            Ok(response) => response,
            Err(_) => {
                return Ok(RemoteFeishuReplyLookupOutcome::Degraded(
                    RemoteFeishuReplyLookupFailure::new(
                        RemoteFeishuReplyLookupFailureStage::Http,
                        None,
                        None,
                    ),
                ));
            }
        };
        Ok(Self::parse_reply_lookup_outcome(&response))
    }

    /// 从 message get 响应中提取 sender/chat 路由信息。
    pub fn parse_message_info(
        response: &RemoteFeishuHttpResponse,
    ) -> AxonResult<Option<RemoteFeishuMessageLookupInfo>> {
        let Some(item) = first_success_item(response)? else {
            return Ok(None);
        };
        Ok(Some(RemoteFeishuMessageLookupInfo {
            sender_id: non_empty_string(item.get("sender").and_then(|sender| sender.get("id"))),
            chat_id: non_empty_string(item.get("chat_id")),
        }))
    }

    /// 规划 chat get 请求。
    pub fn plan_get_chat_request(
        api_base_url: Option<&str>,
        tenant_access_token: &str,
        chat_id: &str,
    ) -> AxonResult<RemoteFeishuHttpRequestPlan> {
        if tenant_access_token.trim().is_empty() || chat_id.trim().is_empty() {
            return Err(AxonError::runtime_failure("Feishu 会话查询输入无效"));
        }
        let base = normalized_base_url(api_base_url.unwrap_or(REMOTE_FEISHU_API_BASE_URL))?;
        let mut url = url::Url::parse(&base)
            .map_err(|_| AxonError::runtime_failure("Feishu 会话查询 URL 无效"))?;
        url.set_path(CHAT_LOOKUP_PATH);
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .map_err(|_| AxonError::runtime_failure("Feishu 会话查询 URL 无法追加路径"))?
            .push(chat_id);
        Ok(RemoteFeishuHttpRequestPlan {
            dispatch_id: CHAT_LOOKUP_DISPATCH_ID.to_owned(),
            url: url.into(),
            method: "GET".to_owned(),
            headers: vec![(
                AUTHORIZATION_HEADER.to_owned(),
                format!("{BEARER_PREFIX}{tenant_access_token}"),
            )],
            body_json: String::new(),
        })
    }

    /// 从 chat get 响应中解析会话类型；业务失败或类型未知时返回 `None`。
    ///
    /// reaction 事件不携带 chat_type，而 Interaction 合同要求 p2p/group 明确，因此需要
    /// 这次补查；返回 `None` 时调用方必须跳过该事件，绝不能把群聊默认记成私聊。
    pub fn parse_chat_type(response: &RemoteFeishuHttpResponse) -> Option<RemoteFeishuChatType> {
        if !(200..300).contains(&response.status) {
            return None;
        }
        let value = serde_json::from_str::<serde_json::Value>(&response.body).ok()?;
        if value.get("code").and_then(serde_json::Value::as_i64) != Some(0) {
            return None;
        }
        let data = value.get("data")?;
        let raw = data
            .get("chat_mode")
            .or_else(|| data.get("chat_type"))
            .and_then(serde_json::Value::as_str);
        match RemoteFeishuChatType::from_feishu_value(raw) {
            RemoteFeishuChatType::Unknown => None,
            chat_type => Some(chat_type),
        }
    }

    /// 查询并解析会话类型。HTTP 失败向上传播（可重试），业务失败返回 `None`（应跳过）。
    pub fn fetch_chat_type_with_http<F>(
        api_base_url: Option<&str>,
        tenant_access_token: &str,
        chat_id: &str,
        mut http: F,
    ) -> AxonResult<Option<RemoteFeishuChatType>>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let request = Self::plan_get_chat_request(api_base_url, tenant_access_token, chat_id)?;
        let response = http(&request)?;
        Ok(Self::parse_chat_type(&response))
    }

    /// 查询并提取父消息文本。查询失败时返回 None，不阻断当前入站消息。
    pub fn fetch_message_text_with_http<F>(
        api_base_url: Option<&str>,
        tenant_access_token: &str,
        message_id: &str,
        mut http: F,
    ) -> AxonResult<Option<String>>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let request =
            Self::plan_get_message_request(api_base_url, tenant_access_token, message_id)?;
        let response = http(&request)?;
        Self::parse_message_text(&response)
    }

    /// 查询并提取目标消息路由信息。
    pub fn fetch_message_info_with_http<F>(
        api_base_url: Option<&str>,
        tenant_access_token: &str,
        message_id: &str,
        mut http: F,
    ) -> AxonResult<Option<RemoteFeishuMessageLookupInfo>>
    where
        F: FnMut(&RemoteFeishuHttpRequestPlan) -> AxonResult<RemoteFeishuHttpResponse>,
    {
        let request =
            Self::plan_get_message_request(api_base_url, tenant_access_token, message_id)?;
        let response = http(&request)?;
        Self::parse_message_info(&response)
    }
}

fn validated_reply_items(
    value: &serde_json::Value,
    http_status: u16,
) -> Result<&[serde_json::Value], RemoteFeishuReplyLookupOutcome> {
    let Some(code) = value.get("code").and_then(serde_json::Value::as_i64) else {
        return Err(degraded_reply_lookup(
            RemoteFeishuReplyLookupFailureStage::Json,
            http_status,
            None,
        ));
    };
    if code != 0 {
        return Err(degraded_reply_lookup(
            RemoteFeishuReplyLookupFailureStage::FeishuCode,
            http_status,
            Some(code),
        ));
    }
    value
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| {
            degraded_reply_lookup(RemoteFeishuReplyLookupFailureStage::Json, http_status, None)
        })
}

fn degraded_reply_lookup(
    stage: RemoteFeishuReplyLookupFailureStage,
    http_status: u16,
    feishu_code: Option<i64>,
) -> RemoteFeishuReplyLookupOutcome {
    RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
        stage,
        Some(http_status),
        feishu_code,
    ))
}

fn reply_text_outcome(
    item: &serde_json::Value,
    http_status: u16,
) -> RemoteFeishuReplyLookupOutcome {
    let message_type = item
        .get("msg_type")
        .or_else(|| item.get("message_type"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let content = item
        .get("body")
        .and_then(|body| body.get("content"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let (Some(message_type), Some(content)) = (message_type, content) else {
        return RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
            RemoteFeishuReplyLookupFailureStage::Content,
            Some(http_status),
            None,
        ));
    };
    if serde_json::from_str::<serde_json::Value>(content).is_err() {
        return RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
            RemoteFeishuReplyLookupFailureStage::Content,
            Some(http_status),
            None,
        ));
    }
    visible_text_from_content(message_type, content).map_or(
        RemoteFeishuReplyLookupOutcome::NoVisibleText,
        RemoteFeishuReplyLookupOutcome::VisibleText,
    )
}

fn first_success_item(
    response: &RemoteFeishuHttpResponse,
) -> AxonResult<Option<serde_json::Value>> {
    if response.status != 200 {
        return Ok(None);
    }
    let value = serde_json::from_str::<serde_json::Value>(&response.body)
        .map_err(|_| AxonError::runtime_failure("Feishu 消息查询响应 JSON 无效"))?;
    if value["code"].as_i64().unwrap_or(0) != 0 {
        return Ok(None);
    }
    Ok(value
        .get("data")
        .and_then(|data| data.get("items"))
        .and_then(|items| items.as_array())
        .and_then(|items| items.first())
        .cloned())
}

fn non_empty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn normalized_base_url(value: &str) -> AxonResult<String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
        return Ok(trimmed.to_owned());
    }
    Err(AxonError::runtime_failure(
        "Feishu API base URL 必须为 http/https",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_lookup_extracts_text_from_message_get_response() {
        let response = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"items":[{"msg_type":"text","body":{"content":"{\"text\":\"父消息\"}"}}]}}"#
                .to_owned(),
        };

        let text = RemoteFeishuMessageLookupRuntime::parse_message_text(&response)
            .expect("parse response");

        assert_eq!(text.as_deref(), Some("父消息"));
    }

    #[test]
    fn message_lookup_extracts_sender_and_chat_from_message_get_response() {
        let response = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"items":[{"sender":{"id":"cli_bot_app"},"chat_id":"oc_bot_chat"}]}}"#
                .to_owned(),
        };

        let info = RemoteFeishuMessageLookupRuntime::parse_message_info(&response)
            .expect("parse response")
            .expect("message info should exist");

        assert_eq!(info.sender_id.as_deref(), Some("cli_bot_app"));
        assert_eq!(info.chat_id.as_deref(), Some("oc_bot_chat"));
    }

    #[test]
    fn chat_lookup_plan_encodes_chat_id_as_one_path_segment() {
        let request = RemoteFeishuMessageLookupRuntime::plan_get_chat_request(
            Some("https://open.feishu.cn"),
            "tenant-token",
            "oc_group_chat",
        )
        .expect("plan chat request");

        assert_eq!(request.method, "GET");
        assert_eq!(
            request.url,
            "https://open.feishu.cn/open-apis/im/v1/chats/oc_group_chat"
        );
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "Authorization" && value == "Bearer tenant-token")
        );
    }

    #[test]
    fn chat_lookup_parses_chat_mode_and_rejects_unknown_or_failed_responses() {
        let p2p = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"chat_mode":"p2p"}}"#.to_owned(),
        };
        let group = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"chat_mode":"group","chat_type":"group"}}"#.to_owned(),
        };
        let unknown = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{}}"#.to_owned(),
        };
        let failed = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":99991663,"msg":"chat not found"}"#.to_owned(),
        };
        let http_error = RemoteFeishuHttpResponse {
            status: 404,
            body: String::new(),
        };

        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_chat_type(&p2p),
            Some(RemoteFeishuChatType::P2p)
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_chat_type(&group),
            Some(RemoteFeishuChatType::Group)
        );
        // 类型未知时必须返回 None：调用方据此跳过该事件，绝不默认成私聊。
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_chat_type(&unknown),
            None
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_chat_type(&failed),
            None
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_chat_type(&http_error),
            None
        );
    }

    #[test]
    fn message_lookup_encodes_message_id_as_one_path_segment() {
        let request = RemoteFeishuMessageLookupRuntime::plan_get_message_request(
            Some("https://open.feishu.cn"),
            "tenant-token",
            "om/a b?#中文",
        )
        .expect("plan request");

        assert_eq!(
            request.url,
            "https://open.feishu.cn/open-apis/im/v1/messages/om%2Fa%20b%3F%23%E4%B8%AD%E6%96%87"
        );
        assert_eq!(
            request.headers,
            vec![("Authorization".to_owned(), "Bearer tenant-token".to_owned())]
        );
    }

    #[test]
    fn reply_lookup_distinguishes_protocol_failures() {
        let status = RemoteFeishuHttpResponse {
            status: 503,
            body: String::new(),
        };
        let feishu_code = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":999}"#.to_owned(),
        };
        let invalid_json = RemoteFeishuHttpResponse {
            status: 200,
            body: "{".to_owned(),
        };
        let invalid_content = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"items":[{"msg_type":"text","body":{"content":"{"}}]}}"#
                .to_owned(),
        };

        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&status),
            RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
                RemoteFeishuReplyLookupFailureStage::Status,
                Some(503),
                None,
            ))
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&feishu_code),
            RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
                RemoteFeishuReplyLookupFailureStage::FeishuCode,
                Some(200),
                Some(999),
            ))
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&invalid_json),
            RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
                RemoteFeishuReplyLookupFailureStage::Json,
                Some(200),
                None,
            ))
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&invalid_content),
            RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
                RemoteFeishuReplyLookupFailureStage::Content,
                Some(200),
                None,
            ))
        );
    }

    #[test]
    fn reply_lookup_rejects_malformed_success_shape() {
        let missing_items = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{}}"#.to_owned(),
        };

        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&missing_items),
            degraded_reply_lookup(RemoteFeishuReplyLookupFailureStage::Json, 200, None)
        );
    }

    #[test]
    fn reply_lookup_distinguishes_empty_and_visible_text() {
        let empty = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"items":[]}}"#.to_owned(),
        };
        let visible = RemoteFeishuHttpResponse {
            status: 200,
            body: r#"{"code":0,"data":{"items":[{"msg_type":"text","body":{"content":"{\"text\":\"父消息\"}"}}]}}"#
                .to_owned(),
        };

        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&empty),
            RemoteFeishuReplyLookupOutcome::NoVisibleText
        );
        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_reply_lookup_outcome(&visible),
            RemoteFeishuReplyLookupOutcome::VisibleText("父消息".to_owned())
        );
    }

    #[test]
    fn reply_lookup_converts_http_error_to_degraded_outcome() {
        let outcome = RemoteFeishuMessageLookupRuntime::fetch_reply_lookup_with_http(
            None,
            "tenant-token",
            "om_parent",
            |_| Err(AxonError::runtime_failure("network secret")),
        )
        .expect("lookup outcome");

        assert_eq!(
            outcome,
            RemoteFeishuReplyLookupOutcome::Degraded(RemoteFeishuReplyLookupFailure::new(
                RemoteFeishuReplyLookupFailureStage::Http,
                None,
                None,
            ))
        );
    }

    #[test]
    fn reaction_lookup_keeps_legacy_status_degradation() {
        let response = RemoteFeishuHttpResponse {
            status: 503,
            body: "{invalid".to_owned(),
        };

        assert_eq!(
            RemoteFeishuMessageLookupRuntime::parse_message_info(&response)
                .expect("legacy degradation"),
            None
        );
    }
}
