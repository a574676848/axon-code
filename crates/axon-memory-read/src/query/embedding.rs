use std::{sync::OnceLock, time::Duration};

use axon_config::{AxonConfig, ModelConfig, ProviderConfig, provider_credential};
use axon_domain::AxonResult;
use axon_memory::{
    MemoryEmbeddingAuthHeaderPlan, MemoryEmbeddingCredentialPlan, MemoryEmbeddingHttpHeader,
    MemoryEmbeddingProviderRuntime, MemoryEmbeddingRequestInput, MemoryEmbeddingRequestPlan,
};
use axon_rustls_provider::ensure_rustls_crypto_provider;

/// Memory 查询 embedding 已生成。
pub const MEMORY_QUERY_EMBEDDING_GENERATED: &str = "generated";
/// Memory 查询 embedding 已禁用。
pub const MEMORY_QUERY_EMBEDDING_DISABLED: &str = "disabled";
/// Memory 查询 embedding 已降级。
pub const MEMORY_QUERY_EMBEDDING_FALLBACK: &str = "fallback";
const MEMORY_EMBEDDING_PREWARM_INPUT: &str = "axon connection prewarm";

/// 非敏感 HTTP header。
pub type MemoryReadHttpHeader = MemoryEmbeddingHttpHeader;
/// 鉴权 header 计划。
pub type MemoryReadAuthHeaderPlan = MemoryEmbeddingAuthHeaderPlan;
/// Memory read 鉴权凭据来源。
pub type MemoryReadCredentialPlan = MemoryEmbeddingCredentialPlan;
/// Memory embedding 请求输入。
pub type MemoryReadEmbeddingRequestInput = MemoryEmbeddingRequestInput;
/// Memory embedding 请求计划。
pub type MemoryReadEmbeddingRequestPlan = MemoryEmbeddingRequestPlan;

/// Memory 查询 embedding 生成报告。
#[derive(Debug, Clone, PartialEq)]
pub struct QueryEmbeddingReport {
    /// 向量数据。
    pub embedding: Option<Vec<f32>>,
    /// 生成状态。
    pub status: &'static str,
    /// 降级或失败原因。
    pub reason: Option<String>,
}

/// 生成 provider-specific embeddings 请求计划。
pub fn plan_memory_embedding(
    input: &MemoryReadEmbeddingRequestInput,
) -> AxonResult<MemoryReadEmbeddingRequestPlan> {
    MemoryEmbeddingProviderRuntime.plan_provider(input)
}

/// 解析 OpenAI-compatible embeddings 响应。
pub fn parse_memory_embedding_response(
    body_text: &str,
    expected_dimension: usize,
) -> AxonResult<Vec<f32>> {
    MemoryEmbeddingProviderRuntime.parse_openai_response(body_text, expected_dimension)
}

pub(crate) fn generate_memory_query_embedding(
    config: &AxonConfig,
    prompt: &str,
    timeout: Duration,
) -> QueryEmbeddingReport {
    if !config.memory.embedding.enabled {
        return QueryEmbeddingReport {
            embedding: None,
            status: MEMORY_QUERY_EMBEDDING_DISABLED,
            reason: None,
        };
    }
    let Some((model, provider)) = resolve_memory_embedding_model(config) else {
        return embedding_fallback("memory embedding model missing", None);
    };
    let plan = match plan_memory_embedding(&MemoryReadEmbeddingRequestInput {
        provider_protocol: provider.protocol.clone(),
        base_url: provider.base_url.clone(),
        model_id: model.id.clone(),
        input_text: prompt.to_owned(),
        api_key_env: provider.api_key_env.clone(),
        api_key: resolve_memory_embedding_api_key(provider),
        expected_dimension: config.memory.embedding.dimension,
    }) {
        Ok(plan) => plan,
        Err(error) => {
            return embedding_fallback(
                "memory embedding request plan invalid",
                Some(format!("{error:?}")),
            );
        }
    };
    let body = match send_memory_embedding_http_request(&plan, timeout) {
        Ok(body) => body,
        Err(error) => {
            return embedding_fallback(
                "memory embedding provider unavailable",
                Some(error.clone()),
            );
        }
    };
    let embedding = match parse_memory_embedding_response(&body, config.memory.embedding.dimension)
    {
        Ok(embedding) => embedding,
        Err(error) => {
            return embedding_fallback(
                "memory embedding response invalid",
                Some(format!("{error:?}")),
            );
        }
    };
    QueryEmbeddingReport {
        embedding: Some(embedding),
        status: MEMORY_QUERY_EMBEDDING_GENERATED,
        reason: None,
    }
}

fn resolve_memory_embedding_model(config: &AxonConfig) -> Option<(&ModelConfig, &ProviderConfig)> {
    let model = config.models.iter().find(|model| {
        config.model_is_enabled(model)
            && model.supports_embedding
            && (model.name == config.memory.embedding.model
                || model.id == config.memory.embedding.model)
    })?;
    let provider = config.enabled_provider(&model.provider)?;
    Some((model, provider))
}

/// 解析 Memory embedding 凭据：显式 `api_key` 优先，其次按 `api_key_env` 读取凭据仓库。
///
/// `.env` 凭据只存在于 Axon 进程内仓库，直接读 `std::env::var` 会漏掉它们。
fn resolve_memory_embedding_api_key(provider: &ProviderConfig) -> Option<String> {
    provider
        .api_key
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            provider
                .api_key_env
                .as_deref()
                .and_then(provider_credential)
        })
}

/// 预热 Memory embedding endpoint 的 DNS、TCP、TLS 与 HTTP 连接。
pub fn prewarm_memory_embedding(config: &AxonConfig, timeout: Duration) -> Result<(), String> {
    if !config.memory.embedding.enabled {
        return Ok(());
    }
    let (model, provider) = resolve_memory_embedding_model(config)
        .ok_or_else(|| "memory embedding model missing".to_owned())?;
    let plan = plan_memory_embedding(&MemoryReadEmbeddingRequestInput {
        provider_protocol: provider.protocol.clone(),
        base_url: provider.base_url.clone(),
        model_id: model.id.clone(),
        input_text: MEMORY_EMBEDDING_PREWARM_INPUT.to_owned(),
        api_key_env: provider.api_key_env.clone(),
        api_key: resolve_memory_embedding_api_key(provider),
        expected_dimension: config.memory.embedding.dimension,
    })
    .map_err(|_| "memory embedding request plan invalid".to_owned())?;
    let mut outbound = memory_embedding_agent()
        .head(&plan.url)
        .config()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build();
    for header in &plan.headers {
        outbound = outbound.header(&header.name, &header.value);
    }
    if let Some(auth_header) = plan.auth_header.as_ref() {
        let api_key = resolve_auth_credential(&auth_header.credential)?;
        outbound = outbound.header(
            &auth_header.header_name,
            format!("{}{}", auth_header.value_prefix, api_key),
        );
    }
    outbound
        .call()
        .map(|_| ())
        .map_err(|error| format!("Memory embedding 预热失败：{error}"))
}

fn send_memory_embedding_http_request(
    plan: &MemoryReadEmbeddingRequestPlan,
    timeout: Duration,
) -> Result<String, String> {
    let mut outbound = memory_embedding_agent()
        .post(&plan.url)
        .config()
        .timeout_global(Some(timeout))
        .build();
    for header in &plan.headers {
        outbound = outbound.header(&header.name, &header.value);
    }
    if let Some(auth_header) = plan.auth_header.as_ref() {
        let api_key = resolve_auth_credential(&auth_header.credential)?;
        outbound = outbound.header(
            &auth_header.header_name,
            format!("{}{}", auth_header.value_prefix, api_key),
        );
    }
    outbound
        .send(&plan.body_text)
        .map_err(|error| error.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|error| format!("Memory embedding 响应读取失败：{error}"))
}

fn memory_embedding_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ensure_rustls_crypto_provider();
        ureq::Agent::config_builder().build().into()
    })
}

fn resolve_auth_credential(credential: &MemoryReadCredentialPlan) -> Result<String, String> {
    match credential {
        MemoryReadCredentialPlan::ApiKey(value) => Ok(value.clone()),
        MemoryReadCredentialPlan::Env { name } => {
            provider_credential(name).ok_or_else(|| format!("API key 环境变量未设置：{name}"))
        }
    }
}

fn embedding_fallback(reason: impl Into<String>, detail: Option<String>) -> QueryEmbeddingReport {
    let reason = reason.into();
    match detail {
        Some(detail) => axon_logger::warning(&format!(
            "Memory embedding 查询降级：reason={reason}; detail={detail}"
        )),
        None => axon_logger::warning(&format!("Memory embedding 查询降级：{reason}")),
    }
    QueryEmbeddingReport {
        embedding: None,
        status: MEMORY_QUERY_EMBEDDING_FALLBACK,
        reason: Some(reason),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    use super::*;

    const TEST_API_KEY: &str = "fixture-api-key";
    const TEST_EMBEDDING_MODEL: &str = "fixture-embedding";

    #[test]
    fn embedding_prewarm_and_query_reuse_authenticated_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || serve_embedding_connection(&listener));
        let config = embedding_config(format!("http://{address}/v1"));

        prewarm_memory_embedding(&config, Duration::from_secs(1)).unwrap();
        let report =
            generate_memory_query_embedding(&config, "cache probe", Duration::from_secs(1));
        let requests = server.join().unwrap();

        assert_eq!(report.status, MEMORY_QUERY_EMBEDDING_GENERATED);
        assert_eq!(report.embedding, Some(vec![0.25, 0.75]));
        assert!(requests[0].starts_with("HEAD /v1/embeddings HTTP/1.1"));
        assert!(requests[1].starts_with("POST /v1/embeddings HTTP/1.1"));
        assert!(requests.iter().all(|request| {
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer fixture-api-key")
        }));
    }

    #[test]
    fn disabled_provider_is_not_an_embedding_candidate() {
        let mut config = embedding_config("https://provider.invalid/v1".to_owned());
        config.providers[0].enabled = false;

        assert!(resolve_memory_embedding_model(&config).is_none());
        assert_eq!(
            prewarm_memory_embedding(&config, Duration::from_secs(1)),
            Err("memory embedding model missing".to_owned())
        );
    }

    #[test]
    fn embedding_api_key_prefers_config_then_provider_credential_store() {
        let env_name = unique_test_env_name("PREFER");
        axon_config::set_provider_credential(&env_name, "store-secret");
        let mut config = embedding_config("https://provider.invalid/v1".to_owned());
        config.providers[0].api_key = None;
        config.providers[0].api_key_env = Some(env_name);

        assert_eq!(
            resolve_memory_embedding_api_key(&config.providers[0]).as_deref(),
            Some("store-secret")
        );

        config.providers[0].api_key = Some("config-secret".to_owned());
        assert_eq!(
            resolve_memory_embedding_api_key(&config.providers[0]).as_deref(),
            Some("config-secret")
        );
    }

    #[test]
    fn embedding_request_uses_env_file_credential_store() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || serve_embedding_connection(&listener));
        let env_name = unique_test_env_name("STORE");
        axon_config::set_provider_credential(&env_name, "store-secret");
        let mut config = embedding_config(format!("http://{address}/v1"));
        config.providers[0].api_key = None;
        config.providers[0].api_key_env = Some(env_name);

        prewarm_memory_embedding(&config, Duration::from_secs(1)).unwrap();
        let report =
            generate_memory_query_embedding(&config, "cache probe", Duration::from_secs(1));
        let requests = server.join().unwrap();

        assert_eq!(report.status, MEMORY_QUERY_EMBEDDING_GENERATED);
        assert!(requests.iter().all(|request| {
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer store-secret")
        }));
    }

    fn unique_test_env_name(suffix: &str) -> String {
        format!("AXON_TEST_EMBEDDING_{suffix}_{}", std::process::id())
    }

    fn embedding_config(base_url: String) -> AxonConfig {
        let mut config = AxonConfig::default();
        let mut provider = ProviderConfig::new("fixture", "openai", base_url);
        provider.api_key = Some(TEST_API_KEY.to_owned());
        config.providers.push(provider);
        config.models.push(ModelConfig {
            name: TEST_EMBEDDING_MODEL.to_owned(),
            id: TEST_EMBEDDING_MODEL.to_owned(),
            provider: "fixture".to_owned(),
            context_window: 0,
            max_output_tokens: None,
            supports_vision: false,
            supports_embedding: true,
            embedding_dimension: 2,
        });
        config.memory.embedding.enabled = true;
        config.memory.embedding.model = TEST_EMBEDDING_MODEL.to_owned();
        config.memory.embedding.dimension = 2;
        config
    }

    fn serve_embedding_connection(listener: &TcpListener) -> Vec<String> {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let head = read_request(&mut reader);
        write_response(reader.get_mut(), "204 No Content", "");
        let post = read_request(&mut reader);
        write_response(
            reader.get_mut(),
            "200 OK",
            r#"{"data":[{"embedding":[0.25,0.75]}]}"#,
        );
        vec![head, post]
    }

    fn read_request(reader: &mut BufReader<TcpStream>) -> String {
        let mut request = String::new();
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line
                .to_ascii_lowercase()
                .strip_prefix("content-length:")
                .and_then(|value| value.trim().parse::<usize>().ok())
            {
                content_length = value;
            }
            request.push_str(&line);
        }
        let mut body = vec![0; content_length];
        reader.read_exact(&mut body).unwrap();
        request.push_str(&String::from_utf8(body).unwrap());
        request
    }

    fn write_response(stream: &mut TcpStream, status: &str, body: &str) {
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        stream.flush().unwrap();
    }
}
