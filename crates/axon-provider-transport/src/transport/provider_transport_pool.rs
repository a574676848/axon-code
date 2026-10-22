//! Provider 进程级异步 runtime 与 HTTP client 连接池。

use std::{sync::OnceLock, time::Duration};

use axon_domain::{AxonError, AxonResult};
use axon_rustls_provider::ensure_rustls_crypto_provider;
use reqwest::Client;
use tokio::runtime::Runtime;

use crate::runtime_limits::{
    PROVIDER_HTTP_CONNECT_TIMEOUT_MS, PROVIDER_HTTP_POOL_IDLE_TIMEOUT_MS,
    PROVIDER_HTTP_TCP_KEEPALIVE_MS,
};

pub(super) fn shared_http_client() -> AxonResult<Client> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(build_http_client)
        .as_ref()
        .cloned()
        .map_err(|_| AxonError::provider_failure("provider HTTP client 初始化失败"))
}

fn build_http_client() -> Result<Client, String> {
    ensure_rustls_crypto_provider();
    reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(PROVIDER_HTTP_CONNECT_TIMEOUT_MS))
        .pool_idle_timeout(Duration::from_millis(PROVIDER_HTTP_POOL_IDLE_TIMEOUT_MS))
        .tcp_keepalive(Duration::from_millis(PROVIDER_HTTP_TCP_KEEPALIVE_MS))
        .http2_adaptive_window(true)
        .build()
        .map_err(|error| error.to_string())
}

pub(super) fn shared_runtime() -> AxonResult<&'static Runtime> {
    static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|_| "provider async runtime 初始化失败".to_owned())
        })
        .as_ref()
        .map_err(|_| AxonError::provider_failure("provider async runtime 初始化失败"))
}

#[cfg(test)]
pub(super) fn shared_http_client_initialized() -> bool {
    shared_http_client().is_ok()
}
