//! 端点解析：文件里的一个档案如何落成可连接的 HTTP 与 WebSocket 根地址。
//!
//! 三级优先级（合同 §9 阶段 0）：
//!
//! ```text
//! 运行时态（生效档案缓存） > 配置文件（Local 档案每次解析都重读两个键） > 兜底常量
//! ```
//!
//! `Local` 档案的 `http_bind` / `websocket_bind` 是**故意**每次解析都重读的：
//! 改完监听地址不重启应用就要生效。为了不把这个「读两个键」变成「每次请求做一次
//! 全量配置解析加校验」，这里走 [`axon_config::raw_string_values`] 的轻路径。
//! 状态文件与凭据**不进**请求路径。

use std::time::Duration;

use axon_config::{ConfigLoader, raw_string_values};

use crate::connection::profile::EndpointSpec;

/// 网关 HTTP 监听地址配置键。
pub const KEY_HTTP_BIND: &str = "remote.gateway.http_bind";
/// 网关 WebSocket 监听地址配置键。
pub const KEY_WEBSOCKET_BIND: &str = "remote.gateway.websocket_bind";
/// 网关凭据环境变量名配置键。
pub const KEY_API_TOKEN_ENV: &str = "remote.gateway.api_token_env";
/// HTTP 监听地址缺失时的兜底值。
pub const FALLBACK_HTTP_BIND: &str = "127.0.0.1:3143";
/// WebSocket 监听地址缺失时的兜底值。
pub const FALLBACK_WEBSOCKET_BIND: &str = "127.0.0.1:3144";
/// 凭据变量名缺失时的兜底值。
pub const FALLBACK_TOKEN_ENV: &str = "AXON_GATEWAY_API_TOKEN";
/// 单次请求超时。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 探活单次预算：档案切换面板的总预算内必须能出结论。
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);

/// 解析后的端点对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedEndpoints {
    /// HTTP 根地址，形如 `http://127.0.0.1:3143`。
    pub http_base: String,
    /// WebSocket 根地址，形如 `ws://127.0.0.1:3144`。
    pub ws_base: String,
}

/// 把通配绑定地址映射成可连接的探测地址。
///
/// `0.0.0.0` 与 `::` 表示「所有网卡」，不能作为连接目标，统一映射到回环地址。
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

/// 读本机档案的两个监听地址。
///
/// 只读两个键、只读一次盘。任一键缺失就落到兜底常量，不因为另一键存在而改变兜底。
#[must_use]
pub fn local_endpoints() -> ResolvedEndpoints {
    let Ok(config_path) = ConfigLoader::default_user_config_path() else {
        return fallback_endpoints();
    };
    let values = raw_string_values(&config_path, &[KEY_HTTP_BIND, KEY_WEBSOCKET_BIND]);
    let http_bind = values
        .first()
        .and_then(Clone::clone)
        .unwrap_or_else(|| FALLBACK_HTTP_BIND.to_owned());
    let ws_bind = values
        .get(1)
        .and_then(Clone::clone)
        .unwrap_or_else(|| FALLBACK_WEBSOCKET_BIND.to_owned());
    ResolvedEndpoints {
        http_base: format!("http://{}", probe_target(&http_bind)),
        ws_base: format!("ws://{}", probe_target(&ws_bind)),
    }
}

/// 兜底端点：配置读不出来时使用。
#[must_use]
pub fn fallback_endpoints() -> ResolvedEndpoints {
    ResolvedEndpoints {
        http_base: format!("http://{}", probe_target(FALLBACK_HTTP_BIND)),
        ws_base: format!("ws://{}", probe_target(FALLBACK_WEBSOCKET_BIND)),
    }
}

/// 按档案规格解析端点。
#[must_use]
pub fn resolve(spec: &EndpointSpec) -> ResolvedEndpoints {
    match spec {
        EndpointSpec::Local => local_endpoints(),
        EndpointSpec::Cloud { http_base, ws_base } => ResolvedEndpoints {
            http_base: http_base.clone(),
            ws_base: ws_base.clone(),
        },
    }
}

/// 当前生效档案的端点；无档案时退回本机默认解析。
#[must_use]
pub fn active_endpoints() -> ResolvedEndpoints {
    match crate::connection::runtime().active() {
        Some(entry) => resolve(&entry.profile.spec),
        None => local_endpoints(),
    }
}

/// 当前生效档案的 HTTP 根地址。
#[must_use]
pub fn http_base() -> String {
    active_endpoints().http_base
}

/// 当前生效档案的 WebSocket 根地址。
#[must_use]
pub fn websocket_base() -> String {
    active_endpoints().ws_base
}

/// 网关凭据环境变量名。
///
/// 本机档案的变量名在启动时解析一次并缓存：凭据名是配置里的稳定事实，把它放进
/// 每次请求的路径只会让「只读两个键」变成「只读三个键」。
#[must_use]
pub fn token_env_name() -> String {
    crate::connection::runtime().token_env_name()
}

/// 网关凭据；未配置时返回 None。
///
/// 分发按生效档案的凭据来源走，绝不混用：本机档案读配置相邻 `.env`（与 daemon
/// 同源），云端档案读系统凭据库或本次会话内存。把本机 token 发给云端是安全问题，
/// 反过来则会让云端档案连不上本机。
#[must_use]
pub fn api_token() -> Option<String> {
    match crate::connection::runtime().credential_source() {
        Some(crate::connection::profile::CredentialSource::LocalEnvFile) | None => {
            env_file_token(&crate::connection::runtime().token_env_name())
        }
        Some(_) => crate::connection::runtime().profile_api_token(),
    }
}

/// 从进程缓存或配置相邻 `.env` 取本机凭据。
fn env_file_token(name: &str) -> Option<String> {
    if let Some(existing) = axon_config::provider_credential(name) {
        return Some(existing);
    }
    if let Ok(config_path) = ConfigLoader::default_user_config_path()
        && let Some(env_path) = config_path.parent().map(|dir| dir.join(".env"))
    {
        let _ = axon_config::load_provider_credentials_from_env_file(&env_path);
    }
    axon_config::provider_credential(name)
}

/// 构造一个使用**平台根证书**的 HTTP 客户端。
///
/// 供应商侧走平台验证器、网关侧若只信编译进二进制的 Mozilla 根，会在企业 TLS
/// 中间盒下出现「模型能调通、网关连不上」。因此网关客户端统一取平台根：
/// 系统信任什么，这里就信任什么。
#[must_use]
pub fn http_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        // 非 2xx 也交回响应对象：网关拒绝请求时会在响应体里给出原因，
        // 把状态码当异常抛出会丢掉这句话，用户只能看到一句无法行动的状态码。
        .http_status_as_error(false)
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .new_agent()
}

/// 云端地址规范化结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudAddress {
    /// 规范化后的 HTTP 根地址，形如 `https://axon.example.com`。
    pub http_base: String,
    /// 由 HTTP 地址推导的 WebSocket 根地址。
    pub ws_base: String,
    /// 是否为明文地址（界面要常驻警示条）。
    ///
    /// 只看协议：`http` 就是明文，`https` 就是加密。主机是回环、私网还是公网都不影响
    /// 这个判定。程序不在网络层做限制，要不要 TLS 由部署方的运维配置决定。
    pub plaintext_private: bool,
}

/// 规范化用户填写的云端地址。
///
/// 允许只填域名：缺协议补 `https`，缺端口按协议补（`https` 补 443，`http` 补 80）。
/// 明文 `http` 与 `https` 都接受：能不能连通、要不要走 TLS 由部署方的网络与运维
/// 策略决定，客户端不做额外的网络层限制。`plaintext_private` 只用于在界面上给一句
/// 提示，不构成拒绝。
///
/// # Errors
///
/// 地址为空、协议不受支持、带路径或查询串时返回中文原因。
pub fn normalize_cloud_address(raw: &str) -> Result<CloudAddress, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("请填写后端地址".to_owned());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };
    // 允许用户直接粘贴 WebSocket 地址：等价于它的 HTTP 形态。
    let normalized = if let Some(rest) = with_scheme.strip_prefix("ws://") {
        format!("http://{rest}")
    } else if let Some(rest) = with_scheme.strip_prefix("wss://") {
        format!("https://{rest}")
    } else {
        with_scheme
    };
    let parsed = url::Url::parse(&normalized).map_err(|_| "地址格式不正确".to_string())?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err("地址协议只支持 http 与 https".to_owned());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "地址里缺少主机名".to_string())?
        .to_owned();
    if parsed.path() != "/" && !parsed.path().is_empty() {
        return Err("地址只填到主机与端口，不要带路径".to_owned());
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("地址里不要带查询串或片段".to_owned());
    }
    let port = parsed
        .port()
        .unwrap_or(if scheme == "https" { 443 } else { 80 });
    let default_port = if scheme == "https" { 443 } else { 80 };
    let authority = if port == default_port {
        host.clone()
    } else {
        format!("{host}:{port}")
    };
    let ws_scheme = if scheme == "https" { "wss" } else { "ws" };
    Ok(CloudAddress {
        http_base: format!("{scheme}://{authority}"),
        ws_base: format!("{ws_scheme}://{authority}"),
        plaintext_private: scheme == "http",
    })
}

/// 把一份 WebSocket 根地址换成另一个端口，主机与协议保持不变。
///
/// 只填一个 HTTP 地址时用后端自报的 WS 端口校正；主机保持用户填写的那一个，
/// 因为后端绑定的是 `0.0.0.0` 这类通配地址，它报出的端口才是可用的部分。
#[must_use]
pub fn ws_base_with_port(ws_base: &str, port: u16) -> String {
    let Ok(mut url) = url::Url::parse(ws_base) else {
        return ws_base.to_owned();
    };
    if url.port() == Some(port) {
        return ws_base.to_owned();
    }
    if url.set_port(Some(port)).is_err() {
        return ws_base.to_owned();
    }
    url.to_string().trim_end_matches('/').to_owned()
}

/// 把 WebSocket 根地址换算成它的 HTTP 形态，供「同端口是否存在事件流路由」的探测使用。
#[must_use]
pub fn ws_base_to_http(ws_base: &str) -> String {
    if let Some(rest) = ws_base.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = ws_base.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        ws_base.to_owned()
    }
}

#[cfg(test)]
#[path = "endpoint_tests.rs"]
mod endpoint_tests;
