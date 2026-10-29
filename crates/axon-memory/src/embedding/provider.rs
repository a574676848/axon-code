//! Memory embedding provider 请求计划与响应解析。

#[path = "multimodal.rs"]
mod multimodal;

use axon_client_identity::{ClientChannel, user_agent};
use axon_domain::{AxonError, AxonResult};
pub use multimodal::{
    MemoryEmbeddingMultimodalContent, MemoryEmbeddingMultimodalRequestInput,
    multimodal_embedding_provider_supported,
};
use multimodal::{multimodal_request_body, validate_multimodal_input};

pub(crate) const MEMORY_EMBEDDING_ARK_ENCODING_FORMAT_FLOAT: &str = "float";
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
/// OpenAI 兼容多模态 embeddings provider 协议（网关多向量返回：index0 文本、index1 图像）。
pub const MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL: &str = "openai-multimodal-embeddings";
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

/// Memory embedding 响应解析出的多向量：primary 为 index 最小的向量（文本路），secondary 为次条（图像路）。
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryEmbeddingResponseVectors {
    /// 首条向量（文本路）。
    pub primary: Vec<f32>,
    /// 第二条向量（图像路）；只有单向量响应时为 None。
    pub secondary: Option<Vec<f32>>,
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
    /// 解析 OpenAI-compatible embeddings 响应；只取 index 最小的向量（写侧与预热路径）。
    pub fn parse_openai_response(
        &self,
        body_text: &str,
        expected_dimension: usize,
    ) -> AxonResult<Vec<f32>> {
        self.parse_openai_vectors(body_text, expected_dimension)
            .map(|vectors| vectors.primary)
    }

    /// 解析 OpenAI-compatible embeddings 响应中的多向量：按 index 升序，最多保留两条。
    ///
    /// 网关多模态实现会把同一请求拆成文本与图像两条向量返回（index 0 文本、index 1 图像），
    /// 这里不做融合，交给检索层分别召回后再合并。
    pub fn parse_openai_vectors(
        &self,
        body_text: &str,
        expected_dimension: usize,
    ) -> AxonResult<MemoryEmbeddingResponseVectors> {
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
        let mut vectors = Vec::new();
        for (index, embedding) in embedding_items(&value)? {
            let mut vector = Vec::with_capacity(embedding.len());
            for item in embedding {
                vector.push(parse_embedding_value(item)?);
            }
            validate_embedding(&vector, expected_dimension)?;
            vectors.push((index, vector));
        }
        vectors.sort_by_key(|(index, _)| *index);
        let mut iter = vectors.into_iter().map(|(_, vector)| vector);
        let primary = iter.next().ok_or_else(|| {
            AxonError::provider_invalid_response(
                MEMORY_EMBEDDING_PROVIDER_OPENAI,
                "Memory embedding 响应缺少 embedding 向量",
            )
        })?;
        Ok(MemoryEmbeddingResponseVectors {
            primary,
            secondary: iter.next(),
        })
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

pub(crate) fn validate_dimension(expected_dimension: usize) -> AxonResult<()> {
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
            | MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL
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
        MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL => Ok(serde_json::json!({
            "model": input.model_id,
            "input": [{
                "type": "text",
                "text": input.input_text.trim(),
            }],
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

/// 收集响应里的所有 embedding 向量：`data[]` 与 `output.embeddings[]` 两种形态都支持。
fn embedding_items(value: &serde_json::Value) -> AxonResult<Vec<(u64, &Vec<serde_json::Value>)>> {
    let mut items = Vec::new();
    if let Some(data) = value.get("data").and_then(serde_json::Value::as_array) {
        collect_embedding_items(data, &mut items);
    } else if let Some(embeddings) = value
        .get("output")
        .and_then(|output| output.get("embeddings"))
        .and_then(serde_json::Value::as_array)
    {
        collect_embedding_items(embeddings, &mut items);
    } else if let Some(embedding) = value
        .get("data")
        .and_then(|data| data.get("embedding"))
        .and_then(serde_json::Value::as_array)
    {
        items.push((0, embedding));
    }
    if items.is_empty() {
        return Err(AxonError::provider_invalid_response(
            MEMORY_EMBEDDING_PROVIDER_OPENAI,
            "Memory embedding 响应缺少 embedding 向量",
        ));
    }
    Ok(items)
}

fn collect_embedding_items<'a>(
    entries: &'a [serde_json::Value],
    items: &mut Vec<(u64, &'a Vec<serde_json::Value>)>,
) {
    for (position, entry) in entries.iter().enumerate() {
        let Some(embedding) = entry.get("embedding").and_then(serde_json::Value::as_array) else {
            continue;
        };
        let index = entry
            .get("index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(position as u64);
        items.push((index, embedding));
    }
}

fn header(name: &str, value: &str) -> MemoryEmbeddingHttpHeader {
    MemoryEmbeddingHttpHeader {
        name: name.to_owned(),
        value: value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_openai_vectors_returns_text_and_image_paths() {
        let runtime = MemoryEmbeddingProviderRuntime;
        let body =
            r#"{"data":[{"index":0,"embedding":[1.0,0.0]},{"index":1,"embedding":[0.0,1.0]}]}"#;
        let vectors = runtime.parse_openai_vectors(body, 2).expect("解析应成功");
        assert_eq!(vectors.primary, vec![1.0, 0.0]);
        assert_eq!(vectors.secondary, Some(vec![0.0, 1.0]));

        let single = r#"{"data":[{"index":0,"embedding":[0.5,0.5]}]}"#;
        let vectors = runtime.parse_openai_vectors(single, 2).expect("解析应成功");
        assert_eq!(vectors.primary, vec![0.5, 0.5]);
        assert!(vectors.secondary.is_none());
        assert_eq!(
            runtime.parse_openai_response(single, 2).unwrap(),
            vec![0.5, 0.5]
        );
    }
}
