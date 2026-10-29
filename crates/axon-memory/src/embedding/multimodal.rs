//! Memory 多模态 embedding 请求体构造与验证。

use axon_domain::{AxonError, AxonResult};
use serde_json::Map;

use super::{
    MEMORY_EMBEDDING_ARK_ENCODING_FORMAT_FLOAT, MEMORY_EMBEDDING_PROVIDER_DASHSCOPE,
    MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL,
    MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL, validate_dimension,
};

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

pub(crate) fn validate_multimodal_input(
    input: &MemoryEmbeddingMultimodalRequestInput,
) -> AxonResult<()> {
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

/// 该 provider 协议是否支持多模态 embedding 请求体。
#[must_use]
pub fn multimodal_embedding_provider_supported(protocol: &str) -> bool {
    matches!(
        protocol,
        MEMORY_EMBEDDING_PROVIDER_DASHSCOPE
            | MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL
            | MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL
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

pub(crate) fn multimodal_request_body(
    input: &MemoryEmbeddingMultimodalRequestInput,
) -> AxonResult<serde_json::Value> {
    match input.provider_protocol.as_str() {
        MEMORY_EMBEDDING_PROVIDER_DASHSCOPE => dashscope_multimodal_body(input),
        MEMORY_EMBEDDING_PROVIDER_VOLCENGINE_ARK_MULTIMODAL => ark_multimodal_body(input),
        MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL => openai_multimodal_body(input),
        _ => Err(AxonError::invalid_input(
            "Memory 多模态 embedding provider 协议不受支持",
        )),
    }
}

fn openai_multimodal_body(
    input: &MemoryEmbeddingMultimodalRequestInput,
) -> AxonResult<serde_json::Value> {
    let mut contents = Vec::new();
    for content in &input.contents {
        contents.extend(openai_multimodal_content_items(content)?);
    }
    if contents.is_empty() {
        return Err(AxonError::EmptyInput);
    }
    Ok(serde_json::json!({
        "model": input.model_id,
        "input": contents,
        "dimensions": input.expected_dimension,
    }))
}

/// 把一条已治理内容拆成 OpenAI 多模态 input 元素：文本与图片各占一条，顺序固定为文本在前。
fn openai_multimodal_content_items(
    content: &MemoryEmbeddingMultimodalContent,
) -> AxonResult<Vec<serde_json::Value>> {
    let mut items = Vec::new();
    if let Some(text) = content
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        items.push(serde_json::json!({ "type": "text", "text": text }));
    }
    if let Some(url) = content
        .image_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
    {
        items.push(serde_json::json!({
            "type": "image_url",
            "image_url": { "url": url },
        }));
    }
    if content
        .video_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|url| !url.is_empty())
    {
        return Err(AxonError::invalid_input(
            "OpenAI 兼容多模态 embedding 不支持视频输入",
        ));
    }
    Ok(items)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryEmbeddingProviderRuntime;

    const TEST_PROTOCOL: &str = MEMORY_EMBEDDING_PROVIDER_OPENAI_MULTIMODAL;

    fn multimodal_input() -> MemoryEmbeddingMultimodalRequestInput {
        MemoryEmbeddingMultimodalRequestInput {
            provider_protocol: TEST_PROTOCOL.to_owned(),
            base_url: "http://gateway.local/v1".to_owned(),
            model_id: "qwen3-vl-embedding".to_owned(),
            contents: vec![MemoryEmbeddingMultimodalContent {
                text: Some("检索图片资料".to_owned()),
                image_url: Some("data:image/png;base64,AAAA".to_owned()),
                video_url: None,
            }],
            api_key_env: None,
            api_key: Some("fixture-key".to_owned()),
            expected_dimension: 2,
            dashscope_enable_fusion: true,
        }
    }

    #[test]
    fn openai_multimodal_body_splits_text_and_image() {
        let plan = MemoryEmbeddingProviderRuntime
            .plan_multimodal_provider(&multimodal_input())
            .expect("plan 应成功");
        assert_eq!(plan.url, "http://gateway.local/v1/embeddings");
        let body: serde_json::Value = serde_json::from_str(&plan.body_text).unwrap();
        assert_eq!(body["model"], "qwen3-vl-embedding");
        assert_eq!(body["input"][0]["type"], "text");
        assert_eq!(body["input"][1]["type"], "image_url");
        assert_eq!(
            body["input"][1]["image_url"]["url"],
            "data:image/png;base64,AAAA"
        );
        assert_eq!(body["dimensions"], 2);
        assert!(body.get("parameters").is_none());
        assert!(body.get("enable_fusion").is_none());
    }

    #[test]
    fn openai_multimodal_rejects_video() {
        let mut input = multimodal_input();
        input.contents[0].video_url = Some("https://example.com/a.mp4".to_owned());
        assert!(
            MemoryEmbeddingProviderRuntime
                .plan_multimodal_provider(&input)
                .is_err()
        );
    }
}
