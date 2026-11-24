//! Provider 结构化输出能力协商。
//!
//! 部分 OpenAI-compatible 网关（例如 DeepSeek-V4-Flash）会拒绝 `json_schema`
//! 严格模式。该模块在 transport 重试循环内把结构化输出降级为 prompt-only，
//! 对任何声明了 schema 的请求默认生效：降级后不再发送 Provider 结构化输出字段，
//! 改由 system prompt 承载 JSON Schema 契约，并把降级结果缓存到
//! provider/协议/模型维度，避免重复触发同样的拒绝。

use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

use serde_json::Value;

use crate::{ProviderStreamProtocol, ProviderStreamingTransportError};

/// 结构化输出 wire 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderStructuredOutputMode {
    /// 原生 JSON Schema 严格模式。
    JsonSchema,
    /// 仅由 system prompt 承载 JSON Schema 契约。
    PromptOnly,
}

/// 单轮请求的结构化输出契约。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderStructuredOutputContract {
    /// 期望的 JSON Schema。
    pub schema: Value,
    /// 当前生效的 wire 模式；调用方只需声明期望模式，能力降级由 transport 协商。
    pub mode: ProviderStructuredOutputMode,
}

/// 一次请求最多允许的结构化输出降级次数。
pub(super) const STRUCTURED_OUTPUT_MAX_DOWNGRADE_ATTEMPTS: usize = 1;

const STRUCTURED_OUTPUT_ERROR_HTTP_STATUS: &[u16] = &[400, 404, 422];
const STRUCTURED_OUTPUT_ERROR_FIELDS: &[&str] = &[
    "json_schema",
    "json schema",
    "response_format",
    "text.format",
    "output_config.format",
    "structured output",
    "structured_output",
];
const STRUCTURED_OUTPUT_ERROR_MARKERS: &[&str] = &[
    "unsupported",
    "not supported",
    "does not support",
    "doesn't support",
    "not allowed",
];

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StructuredOutputCacheKey {
    provider_name: String,
    protocol: &'static str,
    model_id: String,
}

type StructuredOutputCache = HashMap<StructuredOutputCacheKey, ProviderStructuredOutputMode>;

static STRUCTURED_OUTPUT_CACHE: OnceLock<RwLock<StructuredOutputCache>> = OnceLock::new();

/// 单个 provider/协议/模型请求的结构化输出协商状态。
pub(super) struct ProviderStructuredOutputNegotiation {
    key: Option<StructuredOutputCacheKey>,
    schema: Option<Value>,
    mode: ProviderStructuredOutputMode,
    capability_downgraded: bool,
}

impl ProviderStructuredOutputNegotiation {
    /// 创建协商状态；未声明 schema 的请求不参与协商。
    pub(super) fn new(
        provider_name: &str,
        protocol: ProviderStreamProtocol,
        model_id: &str,
        contract: Option<&ProviderStructuredOutputContract>,
    ) -> Self {
        let Some(contract) = contract else {
            return Self {
                key: None,
                schema: None,
                mode: ProviderStructuredOutputMode::JsonSchema,
                capability_downgraded: false,
            };
        };
        let schema = contract.schema.clone();
        if contract.mode == ProviderStructuredOutputMode::PromptOnly {
            return Self {
                key: None,
                schema: Some(schema),
                mode: ProviderStructuredOutputMode::PromptOnly,
                capability_downgraded: false,
            };
        }
        let key = StructuredOutputCacheKey {
            provider_name: provider_name.to_owned(),
            protocol: protocol_code(protocol),
            model_id: model_id.to_owned(),
        };
        let mode = structured_output_cache()
            .read()
            .ok()
            .and_then(|cache| cache.get(&key).copied())
            .unwrap_or(ProviderStructuredOutputMode::JsonSchema);
        Self {
            key: Some(key),
            schema: Some(schema),
            mode,
            capability_downgraded: false,
        }
    }

    /// 返回当前生效的请求契约。
    pub(super) fn contract(&self) -> Option<ProviderStructuredOutputContract> {
        self.schema
            .clone()
            .map(|schema| ProviderStructuredOutputContract {
                schema,
                mode: self.mode,
            })
    }

    /// Provider 拒绝 `json_schema` 时降级为 prompt-only。
    pub(super) fn downgrade_for(&mut self, error: &ProviderStreamingTransportError) -> bool {
        if self.key.is_none()
            || self.mode == ProviderStructuredOutputMode::PromptOnly
            || !is_structured_output_capability_error(error)
        {
            return false;
        }
        self.mode = ProviderStructuredOutputMode::PromptOnly;
        self.capability_downgraded = true;
        true
    }

    /// 降级成功后记住该 provider/协议/模型的结构化输出能力上限。
    pub(super) fn remember_success(&self) {
        if !self.capability_downgraded {
            return;
        }
        let Some(key) = self.key.as_ref() else {
            return;
        };
        if let Ok(mut cache) = structured_output_cache().write() {
            cache.insert(key.clone(), self.mode);
        }
    }
}

fn structured_output_cache() -> &'static RwLock<StructuredOutputCache> {
    STRUCTURED_OUTPUT_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn protocol_code(protocol: ProviderStreamProtocol) -> &'static str {
    match protocol {
        ProviderStreamProtocol::OpenAiResponses => "openai-responses",
        ProviderStreamProtocol::OpenAiCompatible => "openai-chat-completions",
        ProviderStreamProtocol::AnthropicMessages => "anthropic-messages",
    }
}

fn is_structured_output_capability_error(error: &ProviderStreamingTransportError) -> bool {
    if !error
        .http_status
        .is_some_and(|status| STRUCTURED_OUTPUT_ERROR_HTTP_STATUS.contains(&status))
    {
        return false;
    }
    let message = error.message.to_ascii_lowercase();
    let param_matches = serde_json::from_str::<Value>(&error.message)
        .ok()
        .and_then(|value| {
            value
                .pointer("/error/param")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
        })
        .is_some_and(|param| {
            STRUCTURED_OUTPUT_ERROR_FIELDS
                .iter()
                .any(|field| param == *field)
        });
    param_matches
        || (STRUCTURED_OUTPUT_ERROR_FIELDS
            .iter()
            .any(|field| message.contains(field))
            && STRUCTURED_OUTPUT_ERROR_MARKERS
                .iter()
                .any(|marker| message.contains(marker)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANTHROPIC_PROTOCOL: ProviderStreamProtocol = ProviderStreamProtocol::AnthropicMessages;
    const COMPATIBLE_PROTOCOL: ProviderStreamProtocol = ProviderStreamProtocol::OpenAiCompatible;

    #[test]
    fn json_schema_capability_error_downgrades_to_prompt_only() {
        let mut negotiation = negotiation_for(
            "compatible-downgrade",
            COMPATIBLE_PROTOCOL,
            "deepseek-v4-flash",
        );

        assert!(negotiation.downgrade_for(&capability_error()));
        let contract = negotiation.contract().expect("降级后仍保留 schema 契约");
        assert_eq!(contract.mode, ProviderStructuredOutputMode::PromptOnly);
        assert_eq!(contract.schema, test_schema());
    }

    #[test]
    fn anthropic_json_schema_capability_error_downgrades_to_prompt_only() {
        let mut negotiation =
            negotiation_for("anthropic-downgrade", ANTHROPIC_PROTOCOL, "claude-test");

        assert!(negotiation.downgrade_for(&capability_error()));
        assert_eq!(
            negotiation
                .contract()
                .expect("降级后仍保留 schema 契约")
                .mode,
            ProviderStructuredOutputMode::PromptOnly
        );
    }

    #[test]
    fn unrelated_error_keeps_json_schema() {
        let mut negotiation = negotiation_for("unrelated-error", COMPATIBLE_PROTOCOL, "test-model");

        assert!(
            !negotiation.downgrade_for(&ProviderStreamingTransportError::new(
                "connection reset",
                None,
            ))
        );
        assert!(
            !negotiation.downgrade_for(&ProviderStreamingTransportError::new(
                "model not found",
                Some(404),
            ))
        );
        assert_eq!(
            negotiation
                .contract()
                .expect("未降级时保留 schema 契约")
                .mode,
            ProviderStructuredOutputMode::JsonSchema
        );
    }

    #[test]
    fn downgraded_mode_is_cached_after_success() {
        let mut negotiation =
            negotiation_for("cached-downgrade", COMPATIBLE_PROTOCOL, "deepseek-v4-flash");
        assert!(negotiation.downgrade_for(&capability_error()));
        negotiation.remember_success();

        let cached = negotiation_for("cached-downgrade", COMPATIBLE_PROTOCOL, "deepseek-v4-flash");
        assert_eq!(
            cached.contract().expect("缓存命中时保留 schema 契约").mode,
            ProviderStructuredOutputMode::PromptOnly
        );
    }

    #[test]
    fn json_schema_contract_downgrades_without_caller_switch() {
        let contract = ProviderStructuredOutputContract {
            schema: test_schema(),
            mode: ProviderStructuredOutputMode::JsonSchema,
        };
        let mut negotiation = ProviderStructuredOutputNegotiation::new(
            "default-downgrade",
            COMPATIBLE_PROTOCOL,
            "deepseek-v4-flash",
            Some(&contract),
        );

        assert!(negotiation.downgrade_for(&capability_error()));
        assert_eq!(
            negotiation
                .contract()
                .expect("降级后仍保留 schema 契约")
                .mode,
            ProviderStructuredOutputMode::PromptOnly
        );
    }

    #[test]
    fn request_without_schema_never_downgrades() {
        let mut negotiation = ProviderStructuredOutputNegotiation::new(
            "no-schema",
            COMPATIBLE_PROTOCOL,
            "test-model",
            None,
        );

        assert!(!negotiation.downgrade_for(&capability_error()));
        assert!(negotiation.contract().is_none());
    }

    fn negotiation_for(
        provider_name: &str,
        protocol: ProviderStreamProtocol,
        model_id: &str,
    ) -> ProviderStructuredOutputNegotiation {
        let contract = ProviderStructuredOutputContract {
            schema: test_schema(),
            mode: ProviderStructuredOutputMode::JsonSchema,
        };
        ProviderStructuredOutputNegotiation::new(provider_name, protocol, model_id, Some(&contract))
    }

    fn capability_error() -> ProviderStreamingTransportError {
        ProviderStreamingTransportError::new(
            r#"{"error":{"message":"Model DeepSeek-V4-Flash does not support JSON schema output mode","type":"invalid_request_error"}}"#,
            Some(400),
        )
    }

    fn test_schema() -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {"answer": {"type": "string"}},
            "required": ["answer"],
            "additionalProperties": false
        })
    }
}
