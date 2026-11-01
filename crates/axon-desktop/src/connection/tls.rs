//! 平台根证书：网关 HTTP 与 WebSocket 两条链路共用同一套信任来源。
//!
//! 供应商侧走平台验证器，而 `ureq` 与 `tokio-tungstenite` 默认只信任**编译进二进制
//! 的 Mozilla 根**，于是企业 TLS 中间盒下会出现「模型能调通、网关连不上」。这里把
//! 两条链路都切到平台根：系统信任什么，网关客户端就信任什么。
//!
//! 两条链路的工作量并不相同（合同 §3.11）：
//!
//! | 侧 | 做法 |
//! |---|---|
//! | HTTP | `ureq` 的 `RootCerts::PlatformVerifier`，一行级 |
//! | WS | `tokio-tungstenite` 没有对应特性，必须自建 `Connector::Rustls` |
//!
//! `rustls-tls-native-roots` 与平台验证器**不等价**：前者只读根证书、不做平台策略
//! 校验，因此这里不能拿它顶替。

use std::sync::{Arc, OnceLock};

use rustls::ClientConfig;
use rustls_platform_verifier::ConfigVerifierExt as _;

/// 进程内唯一一份平台验证器配置。
///
/// 建一次就够：`ClientConfig` 只读，重复构造不会带来任何收益，反而每次都要重新
/// 装载平台信任库。
/// 装载平台信任库。
pub fn platform_client_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG.get_or_init(build).clone()
}

/// 真正构造一次；失败原因要能直接展示给用户。
fn build() -> Result<Arc<ClientConfig>, String> {
    // 平台验证器要求进程里已安装 crypto provider；桌面端在 run() 的最前面就装好了。
    axon_rustls_provider::ensure_rustls_crypto_provider();
    let config = ClientConfig::with_platform_verifier()
        .map_err(|error| format!("无法装载系统根证书：{error}"))?;
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    #[test]
    fn platform_config_can_be_built_and_reused() {
        // 平台验证器在 macOS 与 Windows 上都能装载；Linux 依赖系统信任库，
        // 构造失败时也必须给出中文原因而不是 panic。
        match super::platform_client_config() {
            Ok(config) => {
                let again = super::platform_client_config().expect("第二次必须复用同一份");
                assert!(std::sync::Arc::ptr_eq(&config, &again), "配置应只构造一次");
            }
            Err(reason) => {
                assert!(!reason.is_empty(), "失败必须给出可展示的中文原因");
            }
        }
    }
}
