//! 供应商与模型发现 HTTP 适配器。
//!
//! 只做一件事：向供应商的模型列表端点发一次只读 GET，把返回的候选模型原样交给
//! 界面勾选。候选永远不会直接写入配置，写盘仍由用户确认后经 `PUT /api/v1/config`
//! 完成。
//!
//! 入参与回执都与桌面端 `discover_models` 同构：入参是命令参数的 camelCase 形式
//! （`provider`、`baseUrl`、`protocol`、`apiKeyEnv`），回执是 `DiscoverModelsResponse`
//! 的 snake_case 字段（`endpoint`、`models[].id`、`models[].name`、
//! `models[].context_window`、`models[].max_output_tokens`）。
//!
//! 密钥边界：本模块只接受环境变量名，密钥由 `axon_config::provider_credential`
//! 在进程内解析，绝不穿过请求体，也不会出现在任何回执或错误信息里。
//!
//! 这是阻塞调用（`ureq` 同步客户端）：网关已保证适配器在阻塞池内执行，
//! 因此这里不再自建线程池。

use std::path::PathBuf;
use std::time::Duration;

use axon_config::{ConfigLoader, provider_credential};
use axon_daemon::{GatewayHttpResponse, GatewayHttpRouteHandler};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// 模型发现路由。
const DISCOVER_PATH: &str = "/api/v1/providers/discover";
/// 单次探测的超时时间，避免请求把阻塞池线程长时间挂住。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 模型列表路径段。
const MODELS_PATH: &str = "models";
/// Anthropic 协议：使用独立的鉴权头与版本头。
const PROTOCOL_ANTHROPIC: &str = "anthropic";
/// 探测请求的 User-Agent。
const USER_AGENT: &str = "Axon";
/// 密钥文件相对配置文件目录的名称。
const ENV_FILE_NAME: &str = ".env";
/// 方法不匹配的错误码，与网关自身的 405 口径同名。
const METHOD_NOT_ALLOWED: &str = "method_not_allowed";
/// 请求体不是合法 JSON 的错误码。
const INVALID_JSON: &str = "invalid_json";
/// 入参不合法的错误码。
const INVALID_REQUEST: &str = "invalid_request";
/// 上游探测失败的错误码。
const DISCOVER_FAILED: &str = "discover_failed";

/// 探测入参：与桌面端 `discover_models` 的命令参数同构（Tauri 侧为 camelCase）。
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DiscoverRequest {
    /// 已登记供应商名称；用于补齐未填写的地址、协议与密钥变量名。
    provider: Option<String>,
    /// 接口基础地址；新建供应商尚未落盘时直接给出。
    base_url: Option<String>,
    /// 通信协议。
    protocol: Option<String>,
    /// API Key 环境变量名。
    api_key_env: Option<String>,
}

/// 探测到的候选模型。
#[derive(Debug, Serialize)]
struct DiscoveredModel {
    /// 上游模型标识。
    id: String,
    /// 可读名称，上游未提供时为空。
    name: Option<String>,
    /// 上游声明的上下文窗口，未声明时为空。
    context_window: Option<u64>,
    /// 上游声明的最大输出 token，未声明时为空。
    max_output_tokens: Option<u64>,
}

/// 探测结果。
#[derive(Debug, Serialize)]
struct DiscoverModelsResponse {
    /// 实际请求的端点地址。
    endpoint: String,
    /// 候选模型列表，按标识升序去重。
    models: Vec<DiscoveredModel>,
}

/// 探测失败的两类原因：入参有问题，还是上游不配合。
enum DiscoverError {
    /// 入参无法用于探测。
    Invalid(String),
    /// 上游请求或响应失败。
    Upstream(String),
}

/// 供应商与模型发现适配器。
pub(crate) struct ProvidersHttpAdapter {
    /// 权威配置文件路径：已登记供应商从这里读取。
    config_path: PathBuf,
}

impl ProvidersHttpAdapter {
    /// 以配置文件路径构造。
    pub(crate) fn new(config_path: impl Into<PathBuf>) -> Self {
        Self {
            config_path: config_path.into(),
        }
    }

    /// `POST /api/v1/providers/discover`。
    fn discover(&self, body: &[u8]) -> GatewayHttpResponse {
        let request = match parse_request(body) {
            Ok(request) => request,
            Err(message) => {
                return GatewayHttpResponse::json(
                    400,
                    &json!({ "error": INVALID_JSON, "message": message }),
                );
            }
        };
        // 与配置读取一致：先加载相邻 .env，否则已配置的密钥会被当成缺失。
        self.ensure_credentials_loaded();
        match self.resolve_and_discover(&request) {
            Ok(response) => match serde_json::to_value(&response) {
                Ok(payload) => GatewayHttpResponse::json(200, &payload),
                Err(error) => GatewayHttpResponse::json(
                    500,
                    &json!({ "error": DISCOVER_FAILED, "message": error.to_string() }),
                ),
            },
            Err(DiscoverError::Invalid(message)) => GatewayHttpResponse::json(
                400,
                &json!({ "error": INVALID_REQUEST, "message": message }),
            ),
            Err(DiscoverError::Upstream(message)) => GatewayHttpResponse::json(
                502,
                &json!({ "error": DISCOVER_FAILED, "message": message }),
            ),
        }
    }

    /// 合并已登记档案与入参，然后发起探测。
    ///
    /// 只有既查不到档案、入参又没给地址时才是真的无法探测；新建供应商时名字还没
    /// 写进配置，此时不该因为「查不到这个档案」直接失败。
    fn resolve_and_discover(
        &self,
        request: &DiscoverRequest,
    ) -> Result<DiscoverModelsResponse, DiscoverError> {
        let mut resolved_base = request.base_url.clone().unwrap_or_default();
        let mut resolved_protocol = request.protocol.clone().unwrap_or_default();
        let mut resolved_key = request.api_key_env.clone();

        if let Some(name) = request
            .provider
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let registered = self.registered_target(name);
            if registered.is_none() && resolved_base.trim().is_empty() {
                return Err(DiscoverError::Invalid(format!("未找到供应商：{name}")));
            }
            if let Some((base, protocol, key)) = registered {
                if resolved_base.trim().is_empty() {
                    resolved_base = base;
                }
                if resolved_protocol.trim().is_empty() {
                    resolved_protocol = protocol;
                }
                if resolved_key.is_none() {
                    resolved_key = key;
                }
            }
        }

        execute_discover(&resolved_base, &resolved_protocol, resolved_key.as_deref())
    }

    /// 从已登记供应商解析出探测所需的连接信息。
    fn registered_target(&self, provider: &str) -> Option<(String, String, Option<String>)> {
        let loaded = ConfigLoader::load_from_path(&self.config_path).ok()?;
        let entry = loaded
            .config
            .providers
            .iter()
            .find(|candidate| candidate.name == provider)?;
        Some((
            entry.base_url.clone(),
            entry.protocol.clone(),
            entry.api_key_env.clone(),
        ))
    }

    /// 加载配置相邻 `.env` 中的凭据。
    fn ensure_credentials_loaded(&self) {
        let Some(parent) = self.config_path.parent() else {
            return;
        };
        let _ = axon_config::load_provider_credentials_from_env_file(&parent.join(ENV_FILE_NAME));
    }
}

impl GatewayHttpRouteHandler for ProvidersHttpAdapter {
    fn handle_request(
        &self,
        method: &str,
        path: &str,
        _query: Option<&str>,
        body: &[u8],
    ) -> Option<GatewayHttpResponse> {
        // 精确匹配：未命中一律交回组合器，不得返回统一 404 截断后续适配器。
        if path != DISCOVER_PATH {
            return None;
        }
        Some(match method {
            "POST" => self.discover(body),
            _ => method_not_allowed(),
        })
    }
}

/// 解析探测入参；空正文视为一个全部缺省的请求。
fn parse_request(body: &[u8]) -> Result<DiscoverRequest, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(DiscoverRequest::default());
    }
    serde_json::from_slice::<DiscoverRequest>(body)
        .map_err(|error| format!("请求体不是合法的探测入参：{error}"))
}

/// 向端点发起一次只读探测。
fn execute_discover(
    base_url: &str,
    protocol: &str,
    api_key_env: Option<&str>,
) -> Result<DiscoverModelsResponse, DiscoverError> {
    let trimmed_base = base_url.trim();
    if trimmed_base.is_empty() {
        return Err(DiscoverError::Invalid(
            "供应商未配置接口地址，无法获取模型列表".to_owned(),
        ));
    }
    let endpoint = models_endpoint(trimmed_base);
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err(DiscoverError::Invalid(
            "接口地址必须以 http:// 或 https:// 开头".to_owned(),
        ));
    }

    let credential = api_key_env
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(provider_credential);

    ensure_rustls_crypto_provider();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .http_status_as_error(false)
        .build()
        .new_agent();

    let mut request = agent
        .get(&endpoint)
        .header("accept", "application/json")
        .header("user-agent", USER_AGENT);
    if let Some(key) = credential.as_deref() {
        request = if protocol.trim() == PROTOCOL_ANTHROPIC {
            request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        } else {
            request.header("authorization", &format!("Bearer {key}"))
        };
    }

    let mut response = request
        .call()
        .map_err(|error| DiscoverError::Upstream(format!("请求模型列表失败：{error}")))?;
    let status = response.status().as_u16();
    if status >= 400 {
        return Err(DiscoverError::Upstream(format!(
            "上游返回状态码 {status}，请检查接口地址与密钥配置"
        )));
    }

    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|error| DiscoverError::Upstream(format!("读取模型列表响应失败：{error}")))?;
    let payload: Value = serde_json::from_str(&text)
        .map_err(|_| DiscoverError::Upstream("上游返回的内容不是合法 JSON".to_owned()))?;

    let models = parse_candidates(&payload);
    if models.is_empty() {
        return Err(DiscoverError::Upstream("上游返回的模型列表为空".to_owned()));
    }
    Ok(DiscoverModelsResponse { endpoint, models })
}

/// 拼接模型列表端点：基地址末尾的斜杠会被忽略。
fn models_endpoint(base_url: &str) -> String {
    let trimmed = base_url.trim().trim_end_matches('/');
    format!("{trimmed}/{MODELS_PATH}")
}

/// 从任意 JSON 形态中取出候选模型数组。
fn candidate_array(payload: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = payload.as_array() {
        return Some(array);
    }
    for key in ["data", "models", "result"] {
        if let Some(array) = payload.get(key).and_then(Value::as_array) {
            return Some(array);
        }
        if let Some(array) = payload
            .get(key)
            .and_then(|nested| nested.get("models"))
            .and_then(Value::as_array)
        {
            return Some(array);
        }
    }
    None
}

/// 从条目中读取第一个可用的字符串字段。
fn first_string(entry: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        entry
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

/// 从条目中读取第一个可用的正整数或数字字符串字段。
fn first_number(entry: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = entry.get(*key)?;
        value.as_u64().or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u64>().ok())
        })
    })
}

/// 把响应体解析为候选模型列表，按标识升序去重。
fn parse_candidates(payload: &Value) -> Vec<DiscoveredModel> {
    let Some(array) = candidate_array(payload) else {
        return Vec::new();
    };
    let mut models = array
        .iter()
        .filter_map(|entry| {
            let id = first_string(entry, &["id", "name", "model", "model_name"])?;
            Some(DiscoveredModel {
                id,
                name: first_string(entry, &["display_name", "displayName", "title"]),
                context_window: first_number(
                    entry,
                    &[
                        "context_window",
                        "contextWindow",
                        "context_length",
                        "max_context_length",
                        "inputTokenLimit",
                    ],
                ),
                max_output_tokens: first_number(
                    entry,
                    &[
                        "max_output_tokens",
                        "maxTokens",
                        "max_output",
                        "outputTokenLimit",
                    ],
                ),
            })
        })
        .collect::<Vec<_>>();
    models.sort_by(|left, right| left.id.cmp(&right.id));
    models.dedup_by(|left, right| left.id == right.id);
    models
}

/// 方法不匹配响应。
fn method_not_allowed() -> GatewayHttpResponse {
    GatewayHttpResponse::json(405, &json!({ "error": METHOD_NOT_ALLOWED }))
}

#[cfg(test)]
#[path = "providers_http_adapter_tests.rs"]
mod providers_http_adapter_tests;
