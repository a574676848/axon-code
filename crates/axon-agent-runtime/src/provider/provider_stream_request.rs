//! Provider streaming HTTP 请求计划合同。
//!
//! 该模块只生成 OpenAI Responses / Chat Completions / Anthropic Messages 的请求 URL、header 与 JSON body；
//! HTTP 执行、鉴权密钥读取、超时和响应读取仍由 CLI / 长驻 Agent Pane transport adapter 负责。

use axon_client_identity::{ClientChannel, user_agent};
use axon_config::{
    ANTHROPIC_REASONING_EFFORT_VALUES, OPENAI_REASONING_EFFORT_VALUES, ProviderCapabilities,
    normalize_reasoning_effort,
};
use axon_domain::{AxonError, AxonResult};
use std::path::PathBuf;

use crate::{
    AgentModelHistorySnapshot, ModelToolDefinition, ProviderPromptCachePlan,
    ProviderStreamProtocol, ProviderStructuredOutputContract,
};

#[path = "provider_stream_request_body.rs"]
mod provider_stream_request_body;
#[path = "provider_stream_tools.rs"]
pub(crate) mod provider_stream_tools;

use provider_stream_request_body::body_for_protocol;
#[cfg(test)]
use provider_stream_request_body::{
    ANTHROPIC_THINKING_DISPLAY_SUMMARIZED, ANTHROPIC_THINKING_TYPE_ADAPTIVE,
    OPENAI_REASONING_INCLUDE_ENCRYPTED_CONTENT, REASONING_SUMMARY_AUTO, SERVICE_TIER_DEFAULT,
};

/// OpenAI-compatible chat completions 路径。
pub const OPENAI_CHAT_COMPLETIONS_PATH: &str = "chat/completions";
/// OpenAI Responses 路径。
pub const OPENAI_RESPONSES_PATH: &str = "responses";
/// Anthropic Messages 路径。
pub const ANTHROPIC_MESSAGES_PATH: &str = "messages";
/// Anthropic Messages API 版本。
pub const ANTHROPIC_MESSAGES_VERSION: &str = "2023-06-01";
/// 请求计划已生成原因。
pub const PROVIDER_STREAMING_HTTP_REASON_REQUEST_PLANNED: &str =
    "provider_streaming_http:request_planned";
/// 请求 body 远控关闭原因。
pub const PROVIDER_STREAMING_HTTP_REASON_REMOTE_BODY_CLOSED: &str =
    "provider_streaming_http:remote_body_closed";
const HTTP_METHOD_POST: &str = "POST";
const HEADER_CONTENT_TYPE: &str = "Content-Type";
const HEADER_USER_AGENT: &str = "User-Agent";
const HEADER_ANTHROPIC_VERSION: &str = "anthropic-version";
const HEADER_AUTHORIZATION: &str = "Authorization";
const HEADER_X_API_KEY: &str = "x-api-key";
const CONTENT_TYPE_JSON: &str = "application/json";
const OPENAI_AUTH_PREFIX: &str = "Bearer ";

/// Provider streaming HTTP 请求计划输入。
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderStreamingHttpRequestInput {
    /// provider 名称。
    pub provider_name: String,
    /// Provider 请求历史、缓存和服务端状态能力。
    pub capabilities: ProviderCapabilities,
    /// 当前模型是否支持图片输入。
    pub supports_vision: bool,
    /// Provider 协议。
    pub protocol: ProviderStreamProtocol,
    /// Provider base URL。
    pub base_url: String,
    /// 模型 ID。
    pub model_id: String,
    /// 本轮消息。
    pub messages: AgentModelHistorySnapshot,
    /// 附件归档根目录；仅允许 Provider 投影读取受控归档引用。
    pub attachment_archive_root: Option<PathBuf>,
    /// 模型可见工具定义。
    pub model_tools: Vec<ModelToolDefinition>,
    /// provider prompt cache 计划。
    pub prompt_cache_plan: ProviderPromptCachePlan,
    /// API key 环境变量名。
    pub api_key_env: Option<String>,
    /// 直接配置的 API key，优先级高于 api_key_env。
    pub api_key: Option<String>,
    /// OpenAI-compatible service_tier；None 或 default 不发送。
    pub service_tier: Option<String>,
    /// reasoning effort；Anthropic 请求层会映射为 thinking level。
    pub reasoning_effort: Option<String>,
    /// reasoning summary；provider 请求层会映射为对应展示字段。
    pub reasoning_summary: Option<String>,
    /// 本轮普通 Coding turn 的可选结构化输出契约与 wire 模式。
    pub structured_output: Option<ProviderStructuredOutputContract>,
    /// Responses 服务端会话状态引用；仅 capability 声明支持时使用。
    pub server_conversation_state_ref: Option<String>,
    /// Anthropic Messages max_tokens。
    pub anthropic_max_tokens: u64,
}

/// 非敏感 HTTP header。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingHttpHeader {
    /// Header 名称。
    pub name: String,
    /// Header 值。
    pub value: String,
}

/// 鉴权 header 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingAuthHeaderPlan {
    /// API key 来源。
    pub credential: ProviderStreamingCredentialPlan,
    /// Header 名称。
    pub header_name: String,
    /// Header 值前缀。
    pub value_prefix: String,
}

/// Provider streaming HTTP 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStreamingHttpRequestPlan {
    /// HTTP 方法。
    pub method: &'static str,
    /// 完整 URL。
    pub url: String,
    /// 非敏感 headers。
    pub headers: Vec<ProviderStreamingHttpHeader>,
    /// 鉴权 header 计划。
    pub auth_header: Option<ProviderStreamingAuthHeaderPlan>,
    /// JSON body 文本。
    pub body_text: String,
    /// 最终 WebSocket 编码是否允许复用服务端响应链。
    pub server_conversation_state_allowed: bool,
    /// 是否允许远控读取完整请求 body。
    pub remote_request_body_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Provider streaming HTTP 请求计划 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct ProviderStreamingHttpRequestRuntime;

impl ProviderStreamingHttpRequestRuntime {
    /// 生成 provider streaming HTTP 请求计划。
    pub fn plan(
        &self,
        input: ProviderStreamingHttpRequestInput,
    ) -> AxonResult<ProviderStreamingHttpRequestPlan> {
        self.plan_with_channel(input, ClientChannel::Cli)
    }

    /// 生成带入口渠道身份的请求计划。
    pub fn plan_with_channel(
        &self,
        input: ProviderStreamingHttpRequestInput,
        client_channel: ClientChannel,
    ) -> AxonResult<ProviderStreamingHttpRequestPlan> {
        validate_input(&input)?;
        let url = endpoint_url(&input.base_url, path_for_protocol(input.protocol));
        let request_body = body_for_protocol(&input)?;
        let body_text = serde_json::to_string(&request_body.body)
            .map_err(|_| AxonError::provider_failure("provider streaming 请求体序列化失败"))?;
        let mut reasons = vec![
            PROVIDER_STREAMING_HTTP_REASON_REQUEST_PLANNED.to_owned(),
            PROVIDER_STREAMING_HTTP_REASON_REMOTE_BODY_CLOSED.to_owned(),
        ];
        reasons.extend(request_body.reasons);

        Ok(ProviderStreamingHttpRequestPlan {
            method: HTTP_METHOD_POST,
            url,
            headers: headers_for_protocol(input.protocol, client_channel),
            auth_header: auth_header_for_protocol(input.protocol, input.api_key_env, input.api_key),
            body_text,
            server_conversation_state_allowed: input.protocol
                == ProviderStreamProtocol::OpenAiResponses
                && input.capabilities.supports_server_conversation_state,
            remote_request_body_allowed: false,
            reasons,
        })
    }
}

fn validate_input(input: &ProviderStreamingHttpRequestInput) -> AxonResult<()> {
    if input.provider_name.trim().is_empty()
        || input.base_url.trim().is_empty()
        || input.model_id.trim().is_empty()
    {
        return Err(AxonError::EmptyInput);
    }
    if input
        .service_tier
        .as_deref()
        .is_some_and(|tier| tier.trim().is_empty())
    {
        return Err(AxonError::invalid_input(
            "provider streaming service_tier 不能为空字符串",
        ));
    }
    if input.messages.is_empty() {
        return Err(AxonError::invalid_input(
            "provider streaming request messages 不能为空",
        ));
    }
    if input.protocol == ProviderStreamProtocol::AnthropicMessages
        && input.anthropic_max_tokens == 0
    {
        return Err(AxonError::invalid_input(
            "Anthropic Messages max_tokens 必须大于 0",
        ));
    }
    if let Some(effort) = input.reasoning_effort.as_deref() {
        validate_reasoning_effort(input.protocol, effort)?;
    }
    Ok(())
}

fn validate_reasoning_effort(protocol: ProviderStreamProtocol, effort: &str) -> AxonResult<()> {
    let Some(effort) = normalize_reasoning_effort(effort) else {
        return Err(AxonError::invalid_input("reasoning_effort 不能为空字符串"));
    };
    let supported = match protocol {
        ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
            OPENAI_REASONING_EFFORT_VALUES
        }
        ProviderStreamProtocol::AnthropicMessages => ANTHROPIC_REASONING_EFFORT_VALUES,
    };
    if supported.contains(&effort) {
        return Ok(());
    }
    let protocol_name = match protocol {
        ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
            "OpenAI"
        }
        ProviderStreamProtocol::AnthropicMessages => "Anthropic",
    };
    Err(AxonError::invalid_input(format!(
        "{protocol_name} reasoning_effort 必须为 {}",
        supported.join("、")
    )))
}

fn endpoint_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let normalized_base = normalize_base_url_with_version(base, path);
    format!("{}/{}", normalized_base, path)
}

/// 自动补齐 base_url 的版本号（如果缺失）。
///
/// 规则：
/// - 如果 base_url 已包含版本号（/v1, /v2 等），直接返回
/// - 对于 OpenAI 协议路径（chat/completions, responses），如果缺少版本号则补齐 /v1
/// - 对于 Anthropic 协议路径（messages），如果缺少版本号则补齐 /v1
fn normalize_base_url_with_version(base_url: &str, path: &str) -> String {
    // 检查是否已包含版本号模式：/v数字
    if base_url.matches("/v").any(|_| true) {
        let after_v = base_url.split("/v").last().unwrap_or("");
        if !after_v.is_empty() && after_v.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return base_url.to_owned();
        }
    }

    // 根据不同协议的路径，补齐对应的版本号
    let needs_v1 = matches!(
        path,
        OPENAI_CHAT_COMPLETIONS_PATH | OPENAI_RESPONSES_PATH | ANTHROPIC_MESSAGES_PATH
    );

    if needs_v1 {
        format!("{}/v1", base_url)
    } else {
        base_url.to_owned()
    }
}

fn path_for_protocol(protocol: ProviderStreamProtocol) -> &'static str {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => OPENAI_RESPONSES_PATH,
        ProviderStreamProtocol::OpenAiCompatible => OPENAI_CHAT_COMPLETIONS_PATH,
        ProviderStreamProtocol::AnthropicMessages => ANTHROPIC_MESSAGES_PATH,
    }
}

fn headers_for_protocol(
    protocol: ProviderStreamProtocol,
    client_channel: ClientChannel,
) -> Vec<ProviderStreamingHttpHeader> {
    let mut headers = vec![
        header(HEADER_USER_AGENT, &user_agent(client_channel)),
        header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON),
    ];
    if protocol == ProviderStreamProtocol::AnthropicMessages {
        headers.push(header(HEADER_ANTHROPIC_VERSION, ANTHROPIC_MESSAGES_VERSION));
    }
    headers
}

fn header(name: &str, value: &str) -> ProviderStreamingHttpHeader {
    ProviderStreamingHttpHeader {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn auth_header_for_protocol(
    protocol: ProviderStreamProtocol,
    api_key_env: Option<String>,
    api_key: Option<String>,
) -> Option<ProviderStreamingAuthHeaderPlan> {
    let credential = match api_key.filter(|value| !value.trim().is_empty()) {
        Some(value) => ProviderStreamingCredentialPlan::ApiKey(value),
        None => ProviderStreamingCredentialPlan::Env { name: api_key_env? },
    };
    Some(match protocol {
        ProviderStreamProtocol::OpenAiResponses | ProviderStreamProtocol::OpenAiCompatible => {
            auth_header(credential, HEADER_AUTHORIZATION, OPENAI_AUTH_PREFIX)
        }
        ProviderStreamProtocol::AnthropicMessages => auth_header(credential, HEADER_X_API_KEY, ""),
    })
}

/// Provider 鉴权凭据来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderStreamingCredentialPlan {
    /// 直接配置的 API key。
    ApiKey(String),
    /// 从环境变量读取 API key。
    Env {
        /// 环境变量名。
        name: String,
    },
}

fn auth_header(
    credential: ProviderStreamingCredentialPlan,
    header_name: &str,
    value_prefix: &str,
) -> ProviderStreamingAuthHeaderPlan {
    ProviderStreamingAuthHeaderPlan {
        credential,
        header_name: header_name.to_owned(),
        value_prefix: value_prefix.to_owned(),
    }
}

#[cfg(test)]
#[path = "provider_stream_request_test_support.rs"]
mod test_support;
#[cfg(test)]
pub(super) use test_support::{
    model_tool, refresh_prompt_cache_plan, request_input, request_prompt_cache_plan, search_tool,
};

#[cfg(test)]
mod auth_tests {
    use super::*;

    #[test]
    fn anthropic_auth_prefers_x_api_key_for_inline_key() {
        let plan = auth_header_for_protocol(
            ProviderStreamProtocol::AnthropicMessages,
            Some("LONGCAT_KEY".to_owned()),
            Some("inline-key".to_owned()),
        )
        .unwrap();
        assert_eq!(plan.header_name, HEADER_X_API_KEY);
        assert_eq!(plan.value_prefix, "");
        assert_eq!(
            plan.credential,
            ProviderStreamingCredentialPlan::ApiKey("inline-key".to_owned())
        );
    }

    #[test]
    fn anthropic_auth_prefers_x_api_key_for_env_key() {
        let plan = auth_header_for_protocol(
            ProviderStreamProtocol::AnthropicMessages,
            Some("LONGCAT_KEY".to_owned()),
            None,
        )
        .unwrap();
        assert_eq!(plan.header_name, HEADER_X_API_KEY);
        assert_eq!(plan.value_prefix, "");
        assert_eq!(
            plan.credential,
            ProviderStreamingCredentialPlan::Env {
                name: "LONGCAT_KEY".to_owned()
            }
        );
    }

    #[test]
    fn other_anthropic_providers_keep_x_api_key_auth() {
        let plan = auth_header_for_protocol(
            ProviderStreamProtocol::AnthropicMessages,
            Some("ANTHROPIC_KEY".to_owned()),
            Some("inline-key".to_owned()),
        )
        .unwrap();
        assert_eq!(plan.header_name, HEADER_X_API_KEY);
        assert_eq!(plan.value_prefix, "");
    }
}
#[cfg(test)]
#[path = "provider_stream_request_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider_stream_request_cache_shape_tests.rs"]
mod cache_shape_tests;

#[cfg(test)]
#[path = "provider_stream_request_archive_image_tests.rs"]
mod archive_image_tests;

#[cfg(test)]
#[path = "provider_stream_request_structured_output_tests.rs"]
mod structured_output_tests;

#[cfg(test)]
#[path = "provider_stream_request_tool_result_tests.rs"]
mod tool_result_tests;

#[cfg(test)]
#[path = "provider_stream_request_freeform_tests.rs"]
mod freeform_tests;
