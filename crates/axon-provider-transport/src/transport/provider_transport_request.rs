//! Provider HTTP 请求构造与请求体编码。

use std::{borrow::Cow, io::Cursor, time::Duration};

use axon_agent_runtime::{
    ProviderStreamingCredentialPlan, ProviderStreamingHttpRequestPlan,
    ProviderStreamingTransportError, ProviderStreamingTransportFailureKind,
    ProviderStreamingTransportOutcome,
};
use axon_config::provider_credential;
use reqwest::{Client, header::CONTENT_ENCODING};
use serde_json::Value;

use super::transport_error;

const PROVIDER_HTTP_ZSTD_COMPRESSION_LEVEL: i32 = 3;
const OPENAI_RESPONSES_PREVIOUS_RESPONSE_ID_FIELD: &str = "previous_response_id";
pub(super) const PROVIDER_HTTP_ZSTD_CONTENT_ENCODING: &str = "zstd";

pub(super) struct ProviderHttpRequestBody {
    bytes: Vec<u8>,
    content_encoding: Option<&'static str>,
}

impl ProviderHttpRequestBody {
    pub(super) fn encode(
        body_text: &str,
        enable_request_compression: bool,
    ) -> Result<Self, ProviderStreamingTransportOutcome> {
        let body_text = stateless_http_body(body_text);
        if !enable_request_compression {
            return Ok(Self {
                bytes: body_text.as_bytes().to_vec(),
                content_encoding: None,
            });
        }
        let bytes = zstd::stream::encode_all(
            Cursor::new(body_text.as_bytes()),
            PROVIDER_HTTP_ZSTD_COMPRESSION_LEVEL,
        )
        .map_err(|error| {
            ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new(
                    format!("provider HTTP 请求体 zstd 压缩失败：{error}"),
                    None,
                )
                .with_failure_kind(ProviderStreamingTransportFailureKind::Request),
            )
        })?;
        Ok(Self {
            bytes,
            content_encoding: Some(PROVIDER_HTTP_ZSTD_CONTENT_ENCODING),
        })
    }
}

fn stateless_http_body(body_text: &str) -> Cow<'_, str> {
    let Ok(Value::Object(mut body)) = serde_json::from_str::<Value>(body_text) else {
        return Cow::Borrowed(body_text);
    };
    if body
        .remove(OPENAI_RESPONSES_PREVIOUS_RESPONSE_ID_FIELD)
        .is_none()
    {
        return Cow::Borrowed(body_text);
    }
    Cow::Owned(Value::Object(body).to_string())
}

pub(super) fn build_request(
    client: &Client,
    request_plan: &ProviderStreamingHttpRequestPlan,
    body: &ProviderHttpRequestBody,
    request_timeout_ms: Option<u64>,
) -> Result<reqwest::RequestBuilder, ProviderStreamingTransportOutcome> {
    let mut outbound = apply_request_headers(client.post(&request_plan.url), request_plan)?;
    if let Some(content_encoding) = body.content_encoding {
        if request_plan
            .headers
            .iter()
            .any(|header| header.name.eq_ignore_ascii_case(CONTENT_ENCODING.as_str()))
        {
            return Err(ProviderStreamingTransportOutcome::Error(
                ProviderStreamingTransportError::new(
                    "provider HTTP 请求压缩与现有 Content-Encoding 冲突",
                    None,
                )
                .with_failure_kind(ProviderStreamingTransportFailureKind::Request),
            ));
        }
        outbound = outbound.header(CONTENT_ENCODING, content_encoding);
    }
    if let Some(timeout_ms) = request_timeout_ms {
        outbound = outbound.timeout(Duration::from_millis(timeout_ms));
    }
    Ok(outbound.body(body.bytes.clone()))
}

pub(super) fn build_prewarm_request(
    client: &Client,
    request_plan: &ProviderStreamingHttpRequestPlan,
    timeout_ms: u64,
) -> Result<reqwest::RequestBuilder, ProviderStreamingTransportOutcome> {
    apply_request_headers(client.head(&request_plan.url), request_plan)
        .map(|outbound| outbound.timeout(Duration::from_millis(timeout_ms)))
}

fn apply_request_headers(
    mut outbound: reqwest::RequestBuilder,
    request_plan: &ProviderStreamingHttpRequestPlan,
) -> Result<reqwest::RequestBuilder, ProviderStreamingTransportOutcome> {
    for header in &request_plan.headers {
        outbound = outbound.header(&header.name, &header.value);
    }
    if let Some(auth_header) = request_plan.auth_header.as_ref() {
        let token = resolve_auth_credential(&auth_header.credential)?;
        outbound = outbound.header(
            &auth_header.header_name,
            format!("{}{}", auth_header.value_prefix, token),
        );
    }
    Ok(outbound)
}

pub(super) fn resolve_auth_credential(
    credential: &ProviderStreamingCredentialPlan,
) -> Result<String, ProviderStreamingTransportOutcome> {
    match credential {
        ProviderStreamingCredentialPlan::ApiKey(value) => Ok(value.clone()),
        ProviderStreamingCredentialPlan::Env { name } => provider_credential(name)
            .ok_or_else(|| transport_error(format!("API key 环境变量未设置：{name}"), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zstd_body_can_be_restored_without_data_loss() {
        let body = ProviderHttpRequestBody::encode(r#"{"model":"gpt-5","stream":true}"#, true)
            .expect("zstd 编码应成功");
        let decoded =
            zstd::stream::decode_all(Cursor::new(body.bytes)).expect("zstd 请求体应可解码");

        assert_eq!(decoded, br#"{"model":"gpt-5","stream":true}"#.to_vec());
        assert_eq!(
            body.content_encoding,
            Some(PROVIDER_HTTP_ZSTD_CONTENT_ENCODING)
        );
    }

    #[test]
    fn http_body_removes_websocket_server_state_reference() {
        for enable_request_compression in [false, true] {
            let body = ProviderHttpRequestBody::encode(
                r#"{"input":[{"role":"user","content":"ping"}],"previous_response_id":"resp-1","stream":true}"#,
                enable_request_compression,
            )
            .expect("HTTP 请求体应可编码");
            let bytes = if enable_request_compression {
                zstd::stream::decode_all(Cursor::new(body.bytes)).expect("zstd HTTP 请求体应可解码")
            } else {
                body.bytes
            };
            let decoded: Value = serde_json::from_slice(&bytes).expect("HTTP 请求体必须有效");

            assert!(
                decoded
                    .get(OPENAI_RESPONSES_PREVIOUS_RESPONSE_ID_FIELD)
                    .is_none()
            );
            assert_eq!(decoded["input"][0]["content"], "ping");
            assert_eq!(decoded["stream"], true);
        }
    }

    #[test]
    fn streaming_request_has_no_total_timeout() {
        let client = Client::new();
        let request_plan = ProviderStreamingHttpRequestPlan {
            method: "POST",
            url: "https://example.com/v1/responses".to_owned(),
            headers: Vec::new(),
            auth_header: None,
            body_text: "{}".to_owned(),
            server_conversation_state_allowed: false,
            remote_request_body_allowed: false,
            reasons: Vec::new(),
        };
        let body = ProviderHttpRequestBody::encode("{}", false).expect("请求体应可编码");

        let streaming = build_request(&client, &request_plan, &body, None)
            .expect("流式请求应可构造")
            .build()
            .expect("流式请求应可生成");
        let bounded = build_request(&client, &request_plan, &body, Some(30_000))
            .expect("有界请求应可构造")
            .build()
            .expect("有界请求应可生成");

        assert_eq!(streaming.timeout(), None);
        assert_eq!(
            bounded.timeout().copied(),
            Some(Duration::from_millis(30_000))
        );
    }
}
