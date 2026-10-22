//! MCP HTTP 共享 blocking client。
//!
//! MCP 的 metadata 与 token 请求都指向同一批授权服务器/资源服务器，复用同一 client
//! 可保留连接池与 TLS 会话，避免每次发现或交换 token 都重新握手。调用方按请求覆盖
//! timeout 即可，无需各自构造 client。

use std::sync::OnceLock;

use axon_domain::{AxonError, AxonResult};
use axon_rustls_provider::ensure_rustls_crypto_provider;

static MCP_HTTP_CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();

/// metadata 发现类请求的默认超时。
///
/// 这些端点返回静态 JSON，超时过长会让多候选回退路径线性放大延迟；5 秒足以覆盖
/// 跨地域 HTTPS 往返，同时保证候选失败时能快速切换。
pub(crate) const MCP_METADATA_TIMEOUT_SECS: u64 = 5;

/// 返回进程级共享 blocking client。
///
/// # Errors
/// TLS provider 或 client 构造失败时返回错误。
pub(crate) fn shared_http_client() -> AxonResult<&'static reqwest::blocking::Client> {
    if let Some(client) = MCP_HTTP_CLIENT.get() {
        return Ok(client);
    }
    ensure_rustls_crypto_provider();
    let client = reqwest::blocking::Client::builder()
        .build()
        .map_err(|_| AxonError::InvalidMcpDefinition("MCP HTTP client 创建失败"))?;
    let _ = MCP_HTTP_CLIENT.set(client);
    MCP_HTTP_CLIENT
        .get()
        .ok_or(AxonError::InvalidMcpDefinition("MCP HTTP client 不可用"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_client_is_reused_across_calls() {
        let first = shared_http_client().expect("共享 client 应可创建");
        let second = shared_http_client().expect("共享 client 应可复用");
        assert!(std::ptr::eq(first, second));
    }
}
