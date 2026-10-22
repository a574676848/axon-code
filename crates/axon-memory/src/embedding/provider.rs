//! Memory embedding provider 请求计划与响应解析。

use axon_client_identity::{ClientChannel, user_agent};
use axon_domain::{AxonError, AxonResult};
use serde_json::Map;

/// OpenAI-compatible embeddings 路径。
pub const MEMORY_EMBEDDING_OPENAI_PATH: &str = "embeddings";
/// 通用 OpenAI-compatible embedding provider 协议。
pub const MEMORY_EMBEDDING_PROVIDER_OPENAI: &str = "openai";
/// OpenAI Chat Completions embedding provider 协议。
pub const MEMORY_EMBEDDING_PROVIDER_OPENAI_CHAT: &str = "openai-chatcompletions";
/// 阿里云百炼多模态 embedding provider 协议。
pub const MEMORY_EMBEDDING_PROVIDER_DASHSCOPE: &str = "dashscope";
/// 火山方舟多模态 embedding provider 协议。
pub const MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL: &str = "volcengine-ark-multimodal";
/// 火山方舟多模态 embeddings 路径。
pub const MEMORY_EMBEDDING_ARK_MULTIMODAL_PATH: &str = "embeddings/multimodal";
/// 阿里云百炼原生多模态 embeddings 路径。
pub const MEMORY_EMBEDDING_DASHSCOPE_PATH: &str =
    "services/embeddings/multimodal-embedding/multimodal-embedding";
/// Memory embedding 请求已生成原因。
pub const MEMORY_EMBEDDING_REASON_REQUEST_PLANNED: &str = "memory_embedding:request_planned";
/// Memory embedding 请求 body 远控关闭原因。
pub const MEMORY_EMBEDDING_REASON_REMOTE_BODY_CLOSED: &str = "memory_embedding:remote_body_closed";
/// Memory embedding 响应原文关闭原因。
pub const MEMORY_EMBEDDING_REASON_RAW_RESPONSE_CLOSED: &str =
    "memory_embedding:raw_response_closed";

const MEMORY_EMBEDDING_ARK_ENCODING_FORMAT_FLOAT: &str = "float";
const HTTP_METHOD_POST: &str = "POST";
const HEADER_CONTENT_TYPE: &str = "Content-Type";
const HEADER_USER_AGENT: &str = "User-Agent";
const HEADER_AUTHORIZATION: &str = "Authorization";
const CONTENT_TYPE_JSON: &str = "application/json";
const OPENAI_AUTH_PREFIX: &str = "Bearer ";

/// 非敏感 HTTP header。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEmbeddingHttpHeader {
    /// Header 名称。
    pub name: String,
    /// Header 值。
    pub value: String,
}

/// 鉴权 header 计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEmbeddingAuthHeaderPlan {
    /// API key 来源。
    pub credential: MemoryEmbeddingCredentialPlan,
    /// Header 名称。
    pub header_name: String,
    /// Header 值前缀。
    pub value_prefix: String,
}

/// Memory embedding 鉴权凭据来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryEmbeddingCredentialPlan {
    /// 直接配置的 API key。
    ApiKey(String),
    /// 从环境变量读取 API key。
    Env {
        /// 环境变量名称。
        name: String,
    },
}

/// Memory embedding 请求输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEmbeddingRequestInput {
    /// Provider 协议。
    pub provider_protocol: String,
    /// Provider base URL。
    pub base_url: String,
    /// Embedding 模型 ID。
    pub model_id: String,
    /// 输入文本；调用方必须传入已治理的查询文本。
    pub input_text: String,
    /// API key 环境变量名。
    pub api_key_env: Option<String>,
    /// 直接配置的 API key，优先级高于 api_key_env。
    pub api_key: Option<String>,
    /// 期望 embedding 维度。
    pub expected_dimension: usize,
}

/// Memory embedding 请求计划。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEmbeddingRequestPlan {
    /// HTTP 方法。
    pub method: &'static str,
    /// 完整 URL。
    pub url: String,
    /// 非敏感 headers。
    pub headers: Vec<MemoryEmbeddingHttpHeader>,
    /// 鉴权 header 计划。
    pub auth_header: Option<MemoryEmbeddingAuthHeaderPlan>,
    /// JSON body 文本。
    pub body_text: String,
    /// 是否允许远控读取完整请求 body。
    pub remote_request_body_allowed: bool,
    /// 是否允许远控读取原始响应。
    pub remote_raw_response_allowed: bool,
    /// 可审计原因。
    pub reasons: Vec<String>,
}

/// Memory 多模态 embedding 请求输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEmbeddingMultimodalRequestInput {
    /// Provider 协议。
    pub provider_protocol: String,
    /// Provider base URL。
    pub base_url: String,
    /// Embedding 模型 ID。
    pub model_id: String,
    /// 已治理的多模态内容引用。
    pub contents: Vec<MemoryEmbeddingMultimodalContent>,
    /// API key 环境变量名。
    pub api_key_env: Option<String>,
    /// 直接配置的 API key，优先级高于 api_key_env。
    pub api_key: Option<String>,
    /// 期望 embedding 维度。
    pub expected_dimension: usize,
    /// 是否按百炼 qwen3-vl-embedding 语义开启融合向量。
    pub dashscope_enable_fusion: bool,
}

/// Memory 多模态 embedding 内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryEmbeddingMultimodalContent {
    /// 文本内容。
    pub text: Option<String>,
    /// 图片 URL 引用。
    pub image_url: Option<String>,
    /// 视频 URL 引用。
    pub video_url: Option<String>,
}

/// Memory embedding provider runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryEmbeddingProviderRuntime;

impl MemoryEmbeddingProviderRuntime {
    /// 生成 provider-specific embeddings 请求计划。
    pub fn plan_provider(
        &self,
        input: &MemoryEmbeddingRequestInput,
    ) -> AxonResult<MemoryEmbeddingRequestPlan> {
        validate_input(input)?;
        let body = request_body(input)?;
        request_plan(
            &input.base_url,
            &input.provider_protocol,
            input.api_key.clone(),
            input.api_key_env.clone(),
            &body,
        )
    }

    /// 生成 OpenAI-compatible embeddings 请求计划。
    pub fn plan_openai(
        &self,
        input: &MemoryEmbeddingRequestInput,
    ) -> AxonResult<MemoryEmbeddingRequestPlan> {
        let mut openai_input = input.clone();
        MEMORY_EMBEDDING_PROVIDER_OPENAI.clone_into(&mut openai_input.provider_protocol);
        self.plan_provider(&openai_input)
    }

    /// 生成 provider-specific 多模态 embeddings 请求计划。
    pub fn plan_multimodal_provider(
        &self,
        input: &MemoryEmbeddingMultimodalRequestInput,
    ) -> AxonResult<MemoryEmbeddingRequestPlan> {
        validate_multimodal_input(input)?;
        let body = multimodal_request_body(input)?;
        request_plan(
            &input.base_url,
            &input.provider_protocol,
            input.api_key.clone(),
            input.api_key_env.clone(),
            &body,
        )
    }

    /// 解析 OpenAI-compatible embeddings 响应。
    pub fn parse_openai_response(
        &self,
        body_text: &str,
        expected_dimension: usize,
    ) -> AxonResult<Vec<f32>> {
        if expected_dimension == 0 {
            return Err(AxonError::invalid_input(
                "Memory embedding 期望维度必须大于 0",
            ));
        }
        let value: serde_json::Value = serde_json::from_str(body_text).map_err(|_| {
            AxonError::provider_invalid_response(
                MEMORY_EMBEDDING_PROVIDER_OPENAI,
                "Memory embedding 响应 JSON 无效",
            )
        })?;
        let embedding = embedding_array(&value)?;
        let mut vector = Vec::with_capacity(embedding.len());
        for value in embedding {
            vector.push(parse_embedding_value(value)?);
        }
        validate_embedding(&vector, expected_dimension)?;
        Ok(vector)
    }
}

fn request_plan(
    base_url: &str,
    provider_protocol: &str,
    api_key: Option<String>,
    api_key_env: Option<String>,
    body: &serde_json::Value,
) -> AxonResult<MemoryEmbeddingRequestPlan> {
    let body_text = serde_json::to_string(&body)
        .map_err(|_| AxonError::runtime_failure("Memory embedding 请求体序列化失败"))?;
    Ok(MemoryEmbeddingRequestPlan {
        method: HTTP_METHOD_POST,
        url: endpoint_url(base_url, provider_protocol),
        headers: vec![
            header(HEADER_USER_AGENT, &user_agent(ClientChannel::Memory)),
            header(HEADER_CONTENT_TYPE, CONTENT_TYPE_JSON),
        ],
        auth_header: auth_header(api_key, api_key_env),
        body_text,
        remote_request_body_allowed: false,
        remote_raw_response_allowed: false,
        reasons: vec![
            MEMORY_EMBEDDING_REASON_REQUEST_PLANNED.to_owned(),
            MEMORY_EMBEDDING_REASON_REMOTE_BODY_CLOSED.to_owned(),
            MEMORY_EMBEDDING_REASON_RAW_RESPONSE_CLOSED.to_owned(),
        ],
    })
}

fn auth_header(
    api_key: Option<String>,
    api_key_env: Option<String>,
) -> Option<MemoryEmbeddingAuthHeaderPlan> {
    let credential = match api_key.filter(|value| !value.trim().is_empty()) {
        Some(value) => MemoryEmbeddingCredentialPlan::ApiKey(value),
        None => MemoryEmbeddingCredentialPlan::Env { name: api_key_env? },
    };
    Some(MemoryEmbeddingAuthHeaderPlan {
        credential,
        header_name: HEADER_AUTHORIZATION.to_owned(),
        value_prefix: OPENAI_AUTH_PREFIX.to_owned(),
    })
}

fn validate_input(input: &MemoryEmbeddingRequestInput) -> AxonResult<()> {
    if input.base_url.trim().is_empty()
        || input.model_id.trim().is_empty()
        || input.input_text.trim().is_empty()
    {
        return Err(AxonError::EmptyInput);
    }
    validate_dimension(input.expected_dimension)?;
    if !embedding_provider_supported(&input.provider_protocol) {
        return Err(AxonError::invalid_input(
            "Memory embedding provider 协议不受支持",
        ));
    }
    Ok(())
}

fn validate_dimension(expected_dimension: usize) -> AxonResult<()> {
    if expected_dimension == 0 {
        return Err(AxonError::invalid_input("Memory embedding 维度必须大于 0"));
    }
    Ok(())
}

fn embedding_provider_supported(protocol: &str) -> bool {
    matches!(
        protocol,
        MEMORY_EMBEDDING_PROVIDER_OPENAI
            | MEMORY_EMBEDDING_PROVIDER_OPENAI_CHAT
            | MEMORY_EMBEDDING_PROVIDER_DASHSCOPE
            | MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL
    )
}

fn request_body(input: &MemoryEmbeddingRequestInput) -> AxonResult<serde_json::Value> {
    match input.provider_protocol.as_str() {
        MEMORY_EMBEDDING_PROVIDER_OPENAI | MEMORY_EMBEDDING_PROVIDER_OPENAI_CHAT => {
            Ok(serde_json::json!({
                "model": input.model_id,
                "input": input.input_text.trim(),
            }))
        }
        MEMORY_EMBEDDING_PROVIDER_DASHSCOPE => Ok(serde_json::json!({
            "model": input.model_id,
            "input": {
                "contents": [{
                    "text": input.input_text.trim(),
                }],
            },
            "parameters": {
                "dimension": input.expected_dimension,
            },
        })),
        MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL => Ok(serde_json::json!({
            "model": input.model_id,
            "input": [{
                "type": "text",
                "text": input.input_text.trim(),
            }],
            "encoding_format": MEMORY_EMBEDDING_ARK_ENCODING_FORMAT_FLOAT,
            "dimensions": input.expected_dimension,
        })),
        _ => Err(AxonError::invalid_input(
            "Memory embedding provider 协议不受支持",
        )),
    }
}

fn parse_embedding_value(value: &serde_json::Value) -> AxonResult<f32> {
    let number = serde_json::from_value::<f32>(value.clone()).map_err(|_| {
        AxonError::provider_invalid_response(
            MEMORY_EMBEDDING_PROVIDER_OPENAI,
            "Memory embedding 响应包含非数字向量值",
        )
    })?;
    if !number.is_finite() {
        return Err(AxonError::provider_invalid_response(
            MEMORY_EMBEDDING_PROVIDER_OPENAI,
            "Memory embedding 响应包含非法数值",
        ));
    }
    Ok(number)
}

fn validate_embedding(embedding: &[f32], expected_dimension: usize) -> AxonResult<()> {
    if embedding.len() != expected_dimension {
        return Err(AxonError::provider_invalid_response(
            MEMORY_EMBEDDING_PROVIDER_OPENAI,
            "Memory embedding 响应维度与配置不一致",
        ));
    }
    if embedding.iter().any(|value| !value.is_finite()) {
        return Err(AxonError::provider_invalid_response(
            MEMORY_EMBEDDING_PROVIDER_OPENAI,
            "Memory embedding 响应包含非法数值",
        ));
    }
    if embedding.iter().all(|value| *value == 0.0) {
        return Err(AxonError::provider_invalid_response(
            MEMORY_EMBEDDING_PROVIDER_OPENAI,
            "Memory embedding 响应不能是全零向量",
        ));
    }
    Ok(())
}

fn endpoint_url(base_url: &str, protocol: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        endpoint_path_for_protocol(protocol)
    )
}

fn endpoint_path_for_protocol(protocol: &str) -> &'static str {
    match protocol {
        MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL => MEMORY_EMBEDDING_ARK_MULTIMODAL_PATH,
        MEMORY_EMBEDDING_PROVIDER_DASHSCOPE => MEMORY_EMBEDDING_DASHSCOPE_PATH,
        _ => MEMORY_EMBEDDING_OPENAI_PATH,
    }
}

fn embedding_array(value: &serde_json::Value) -> AxonResult<&Vec<serde_json::Value>> {
    if let Some(embedding) = value
        .get("data")
        .and_then(|data| data.as_array())
        .and_then(|data| data.first())
        .and_then(|item| item.get("embedding"))
        .and_then(|embedding| embedding.as_array())
    {
        return Ok(embedding);
    }
    if let Some(embedding) = value
        .get("output")
        .and_then(|output| output.get("embeddings"))
        .and_then(|embeddings| embeddings.as_array())
        .and_then(|embeddings| embeddings.first())
        .and_then(|item| item.get("embedding"))
        .and_then(|embedding| embedding.as_array())
    {
        return Ok(embedding);
    }
    value
        .get("data")
        .and_then(|data| data.get("embedding"))
        .and_then(|embedding| embedding.as_array())
        .ok_or_else(|| {
            AxonError::provider_invalid_response(
                MEMORY_EMBEDDING_PROVIDER_OPENAI,
                "Memory embedding 响应缺少 embedding 向量",
            )
        })
}

fn header(name: &str, value: &str) -> MemoryEmbeddingHttpHeader {
    MemoryEmbeddingHttpHeader {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

fn validate_multimodal_input(input: &MemoryEmbeddingMultimodalRequestInput) -> AxonResult<()> {
    if input.base_url.trim().is_empty() || input.model_id.trim().is_empty() {
        return Err(AxonError::EmptyInput);
    }
    validate_dimension(input.expected_dimension)?;
    if !multimodal_embedding_provider_supported(&input.provider_protocol) {
        return Err(AxonError::invalid_input(
            "Memory 多模态 embedding provider 协议不受支持",
        ));
    }
    if input.contents.is_empty() {
        return Err(AxonError::EmptyInput);
    }
    for content in &input.contents {
        validate_multimodal_content(content)?;
    }
    Ok(())
}

fn multimodal_embedding_provider_supported(protocol: &str) -> bool {
    matches!(
        protocol,
        MEMORY_EMBEDDING_PROVIDER_DASHSCOPE | MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL
    )
}

fn validate_multimodal_content(content: &MemoryEmbeddingMultimodalContent) -> AxonResult<()> {
    let has_text = content
        .text
        .as_ref()
        .is_some_and(|text| !text.trim().is_empty());
    let has_image = content
        .image_url
        .as_ref()
        .is_some_and(|url| !url.trim().is_empty());
    let has_video = content
        .video_url
        .as_ref()
        .is_some_and(|url| !url.trim().is_empty());
    if !has_text && !has_image && !has_video {
        return Err(AxonError::EmptyInput);
    }
    if let Some(url) = content.image_url.as_deref() {
        validate_governed_media_url(url)?;
    }
    if let Some(url) = content.video_url.as_deref() {
        validate_governed_media_url(url)?;
    }
    Ok(())
}

fn validate_governed_media_url(url: &str) -> AxonResult<()> {
    let url = url.trim();
    if url.starts_with("https://") || url.starts_with("data:image/") {
        return Ok(());
    }
    Err(AxonError::invalid_input(
        "Memory 多模态 embedding 只接受 https URL 或 data:image/ base64 编码图片",
    ))
}

fn multimodal_request_body(
    input: &MemoryEmbeddingMultimodalRequestInput,
) -> AxonResult<serde_json::Value> {
    match input.provider_protocol.as_str() {
        MEMORY_EMBEDDING_PROVIDER_DASHSCOPE => dashscope_multimodal_body(input),
        MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL => ark_multimodal_body(input),
        _ => Err(AxonError::invalid_input(
            "Memory 多模态 embedding provider 协议不受支持",
        )),
    }
}

fn dashscope_multimodal_body(
    input: &MemoryEmbeddingMultimodalRequestInput,
) -> AxonResult<serde_json::Value> {
    let contents = input
        .contents
        .iter()
        .map(dashscope_content_value)
        .collect::<AxonResult<Vec<_>>>()?;
    let mut parameters = Map::new();
    parameters.insert(
        "dimension".to_owned(),
        serde_json::Value::from(input.expected_dimension),
    );
    if input.dashscope_enable_fusion {
        parameters.insert("enable_fusion".to_owned(), serde_json::Value::Bool(true));
    }

    Ok(serde_json::json!({
        "model": input.model_id,
        "input": {
            "contents": contents,
        },
        "parameters": parameters,
    }))
}

fn dashscope_content_value(
    content: &MemoryEmbeddingMultimodalContent,
) -> AxonResult<serde_json::Value> {
    let mut value = Map::new();
    if let Some(text) = content
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        value.insert(
            "text".to_owned(),
            serde_json::Value::String(text.to_owned()),
        );
    }
    if let Some(url) = content
        .image_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        value.insert(
            "image".to_owned(),
            serde_json::Value::String(url.to_owned()),
        );
    }
    if let Some(url) = content
        .video_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        value.insert(
            "video".to_owned(),
            serde_json::Value::String(url.to_owned()),
        );
    }
    if value.is_empty() {
        return Err(AxonError::EmptyInput);
    }
    Ok(serde_json::Value::Object(value))
}

fn ark_multimodal_body(
    input: &MemoryEmbeddingMultimodalRequestInput,
) -> AxonResult<serde_json::Value> {
    let mut items = Vec::new();
    for content in &input.contents {
        push_ark_content_items(content, &mut items);
    }
    if items.is_empty() {
        return Err(AxonError::EmptyInput);
    }
    Ok(serde_json::json!({
        "model": input.model_id,
        "input": items,
        "encoding_format": MEMORY_EMBEDDING_ARK_ENCODING_FORMAT_FLOAT,
        "dimensions": input.expected_dimension,
    }))
}

fn push_ark_content_items(
    content: &MemoryEmbeddingMultimodalContent,
    items: &mut Vec<serde_json::Value>,
) {
    if let Some(text) = content
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        items.push(serde_json::json!({
            "type": "text",
            "text": text,
        }));
    }
    if let Some(url) = content
        .image_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        items.push(serde_json::json!({
            "type": "image_url",
            "image_url": {
                "url": url,
            },
        }));
    }
    if let Some(url) = content
        .video_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        items.push(serde_json::json!({
            "type": "video_url",
            "video_url": {
                "url": url,
            },
        }));
    }
}
