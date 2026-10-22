//! 网关客户端：桌面端访问 Axon 运行态的唯一入口。
//!
//! 桌面端不直连运行态数据库，也不链接会话子系统，一律经网关 HTTP 协议读写，
//! 从而与后端实现解耦。网关地址与凭据都取自权威配置：
//! 监听地址读 `remote.gateway.http_bind`，凭据名读 `remote.gateway.api_token_env`，
//! 凭据值从配置相邻的 `.env` 加载。
//!
//! 通配绑定地址（`0.0.0.0`、`::`）表示「所有网卡」，不能作为连接目标，
//! 这里统一映射到回环地址后再发起请求。
//!
//! 网关有**两个协议面**：工作台扩展面以 [`API_PREFIX`] 为前缀，交互事件面则是
//! `/gateway/v1/*`。两者共用同一份地址与凭据，因此地址组装只在这里实现一次。

use std::time::Duration;

use axon_config::{ConfigLoader, get_config_value};

/// 网关 HTTP 监听地址配置键。
pub const KEY_HTTP_BIND: &str = "remote.gateway.http_bind";
/// 网关 WebSocket 监听地址配置键。
pub const KEY_WEBSOCKET_BIND: &str = "remote.gateway.websocket_bind";
/// 网关凭据环境变量名配置键。
pub const KEY_API_TOKEN_ENV: &str = "remote.gateway.api_token_env";
/// HTTP 监听地址缺失时的兜底值。
const FALLBACK_HTTP_BIND: &str = "127.0.0.1:3143";
/// WebSocket 监听地址缺失时的兜底值。
const FALLBACK_WEBSOCKET_BIND: &str = "127.0.0.1:3144";
/// 凭据变量名缺失时的兜底值。
const FALLBACK_TOKEN_ENV: &str = "AXON_GATEWAY_API_TOKEN";
/// 单次请求超时。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 把通配绑定地址映射成可连接的探测地址。
#[must_use]
pub fn probe_target(bind: &str) -> String {
    let trimmed = bind.trim();
    if let Some(port) = trimmed.strip_prefix("0.0.0.0:") {
        return format!("127.0.0.1:{port}");
    }
    if let Some(port) = trimmed.strip_prefix("[::]:") {
        return format!("[::1]:{port}");
    }
    if let Some(port) = trimmed.strip_prefix("::") {
        return format!("[::1]:{port}");
    }
    trimmed.to_string()
}

/// 读取配置项文本值。
pub fn config_text(key: &str) -> Option<String> {
    let path = ConfigLoader::default_user_config_path().ok()?;
    let loaded = ConfigLoader::load_from_path(&path).ok()?;
    get_config_value(&loaded.config, key).map(|value| value.to_string())
}

/// 网关根地址，形如 `http://127.0.0.1:3143`。
pub fn base_url() -> Result<String, String> {
    let bind = config_text(KEY_HTTP_BIND).unwrap_or_else(|| FALLBACK_HTTP_BIND.to_string());
    Ok(format!("http://{}", probe_target(&bind)))
}

/// 网关 WebSocket 根地址，形如 `ws://127.0.0.1:3144`。
pub fn websocket_base_url() -> Result<String, String> {
    let bind =
        config_text(KEY_WEBSOCKET_BIND).unwrap_or_else(|| FALLBACK_WEBSOCKET_BIND.to_string());
    Ok(format!("ws://{}", probe_target(&bind)))
}

/// 网关凭据；未配置时返回 None。
///
/// 与供应商凭据同源：先把配置相邻 `.env` 加载进进程缓存，再按配置声明的
/// 变量名取值，因此凭据只存在一份，不在桌面端另建存储。
pub fn api_token() -> Option<String> {
    let name = config_text(KEY_API_TOKEN_ENV).unwrap_or_else(|| FALLBACK_TOKEN_ENV.to_string());
    if let Some(existing) = axon_config::provider_credential(&name) {
        return Some(existing);
    }
    let config_path = ConfigLoader::default_user_config_path().ok()?;
    let env_path = config_path.parent()?.join(".env");
    let _ = axon_config::load_provider_credentials_from_env_file(&env_path);
    axon_config::provider_credential(&name)
}

/// 构造带鉴权的请求代理。
fn agent() -> ureq::Agent {
    agent_with_timeout(REQUEST_TIMEOUT)
}

fn agent_with_timeout(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // 非 2xx 也交回响应对象：工作台拒绝请求时会在响应体里给出原因
        // （例如「该作业不属于这条车道」），把状态码当异常抛出会丢掉这句话，
        // 用户只能看到一句无法行动的状态码。
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// 网关请求共用的路径前缀。
const API_PREFIX: &str = "/api/v1";

/// 组装完整地址。
fn endpoint(path: &str) -> Result<String, String> {
    Ok(format!("{}{API_PREFIX}{path}", base_url()?))
}

/// 组装不经工作台前缀的完整地址。
///
/// 交互事件面（`/gateway/v1/*`）与工作台面同源同凭据，但不共用前缀，
/// 因此这里显式接受完整路径，避免调用方各自拼字符串。
pub fn endpoint_at(path: &str) -> Result<String, String> {
    Ok(format!("{}{path}", base_url()?))
}

/// 构造带鉴权的 GET 请求。
fn authorized_get(url: &str) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    authorized_get_with_timeout(url, REQUEST_TIMEOUT)
}

fn authorized_get_with_timeout(
    url: &str,
    timeout: Duration,
) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
    let mut request = agent_with_timeout(timeout)
        .get(url)
        .header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    request
}

/// GET 一个网关接口并解析 JSON。
pub fn get_json(path: &str) -> Result<serde_json::Value, String> {
    let (status, body) = get_raw(path)?;
    if !(200..300).contains(&status) {
        return Err(format!("网关返回状态码 {status}"));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// GET 一个网关接口，返回状态码与响应体。
///
/// 探测需要区分状态码：401 是凭据问题，404 是服务端不含该接口（版本过旧），
/// 二者修复动作完全不同，必须分别归类而不是笼统报「接口调用失败」。
pub fn get_raw(path: &str) -> Result<(u16, String), String> {
    get_raw_with_timeout(path, REQUEST_TIMEOUT)
}

/// GET 一个网关接口，并使用调用方指定的总超时。
pub fn get_raw_with_timeout(path: &str, timeout: Duration) -> Result<(u16, String), String> {
    let url = endpoint(path)?;
    let request = authorized_get_with_timeout(&url, timeout);
    match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            Ok((status, body))
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(format!("网关不可达：{error}")),
    }
}

/// 探活失败的原因分类：决定桌面端给用户哪一句话、要不要重试。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeFailure {
    /// 请求在预算内没有完成：网关可能正在排队，也可能是连接挂住。
    Timeout,
    /// 连接层失败：端口无人接受连接，或链路根本不通。
    Unreachable,
    /// 网关已经应答，但响应体没能读完。
    BodyRead,
}

/// 带失败分类的 GET。
///
/// 口径与 [`get_raw_with_timeout`] 完全一致，只是把传输层错误分成「超时」与
/// 「不可达」两类：二者的修复动作不同，笼统报一句不可达会把排队误报成宕机。
pub fn probe_raw_with_timeout(
    path: &str,
    timeout: Duration,
) -> Result<(u16, String), ProbeFailure> {
    let url = endpoint(path).map_err(|_| ProbeFailure::Unreachable)?;
    match authorized_get_with_timeout(&url, timeout).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            response
                .into_body()
                .read_to_string()
                .map(|body| (status, body))
                .map_err(|_| ProbeFailure::BodyRead)
        }
        Err(ureq::Error::StatusCode(status)) => Ok((status, String::new())),
        Err(error) => Err(classify_transport_failure(&error)),
    }
}

/// 把传输层错误归到探活的两类失败上；读体失败已在上游单独归类。
fn classify_transport_failure(error: &ureq::Error) -> ProbeFailure {
    match error {
        ureq::Error::Timeout(_) | ureq::Error::BodyStalled => ProbeFailure::Timeout,
        ureq::Error::Io(io_error)
            if matches!(
                io_error.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            ProbeFailure::Timeout
        }
        _ => ProbeFailure::Unreachable,
    }
}

/// GET 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
pub fn get_json_at(path: &str) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let (status, body) = match authorized_get(&url).call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// POST 一个网关接口并解析 JSON。
pub fn post_json(path: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = endpoint(path)?;
    let mut request = agent()
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.send(payload.to_string()) {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达或拒绝请求：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// DELETE 一个网关接口并解析 JSON。
pub fn delete_json(path: &str) -> Result<serde_json::Value, String> {
    let url = endpoint(path)?;
    let mut request = agent().delete(&url).header("accept", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.call() {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达或拒绝请求：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// POST 一个绝对路径接口，要求 2xx 并把响应解析为 JSON。
pub fn post_json_at(path: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
    let url = endpoint_at(path)?;
    let mut request = agent()
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(token) = api_token() {
        request = request.header("authorization", &format!("Bearer {token}"));
    }
    let (status, body) = match request.send(payload.to_string()) {
        Ok(response) => {
            let status = response.status().as_u16();
            let body = response
                .into_body()
                .read_to_string()
                .map_err(|error| format!("读取网关响应失败：{error}"))?;
            (status, body)
        }
        Err(ureq::Error::StatusCode(status)) => (status, String::new()),
        Err(error) => return Err(format!("网关不可达或拒绝请求：{error}")),
    };
    if !(200..300).contains(&status) {
        return Err(gateway_status_message(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "网关响应不是合法 JSON".to_string())
}

/// 把非 2xx 响应翻译成可执行的中文说明。
///
/// 401 是凭据问题、404 是服务端不含该接口（版本过旧），二者修复动作完全不同，
/// 不能笼统报「调用失败」；网关自身的错误码原样附在后面，便于定位。
fn gateway_status_message(status: u16, body: &str) -> String {
    let code = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value.get("error").and_then(|error| {
                error.as_str().map(str::to_owned).or_else(|| {
                    error
                        .get("code")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned)
                })
            })
        })
        .unwrap_or_default();
    let suffix = if code.is_empty() {
        String::new()
    } else {
        format!("（{code}）")
    };
    match status {
        401 | 403 => format!("网关拒绝访问：凭据缺失或不匹配{suffix}"),
        404 => format!("网关不含该接口，后台服务版本可能过旧{suffix}"),
        429 => format!("网关繁忙，请稍后重试{suffix}"),
        _ => format!("网关返回状态码 {status}{suffix}"),
    }
}
