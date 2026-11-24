//! rustls 进程级 crypto provider 初始化。

use std::sync::Once;

const REQUIRED_SIGNATURE_SCHEME: rustls::SignatureScheme =
    rustls::SignatureScheme::ECDSA_NISTP521_SHA512;

/// 确保进程内已安装 rustls crypto provider。
///
/// Axon 明确采用 aws-lc-rs provider，与 Codex 保持一致，以覆盖企业 TLS 代理
/// 常见的 ECDSA P-521/SHA-512 证书链。
///
/// # Panics
///
/// 当默认 provider 已声明安装成功但随后无法读取，或安装后的 provider 不支持
/// Axon 要求的签名算法时会 panic。此处属于进程级 TLS 底座校验，失败应尽早暴露。
pub fn ensure_rustls_crypto_provider() {
    static RUSTLS_PROVIDER_INIT: Once = Once::new();
    RUSTLS_PROVIDER_INIT.call_once(|| {
        if rustls::crypto::aws_lc_rs::default_provider()
            .install_default()
            .is_err()
        {
            return;
        }

        let Some(provider) = rustls::crypto::CryptoProvider::get_default() else {
            panic!("aws-lc-rs rustls crypto provider should be installed");
        };
        assert!(
            provider_supports_required_signature_scheme(provider),
            "installed rustls crypto provider must support {REQUIRED_SIGNATURE_SCHEME:?}"
        );
    });
}

fn provider_supports_required_signature_scheme(provider: &rustls::crypto::CryptoProvider) -> bool {
    provider
        .signature_verification_algorithms
        .supported_schemes()
        .contains(&REQUIRED_SIGNATURE_SCHEME)
}
