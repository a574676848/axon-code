//! 自签证书 fixture：证明「证书有问题」与「连不上」是两条不同结论。
//!
//! 三类结论发生在不同阶段（TLS 在 HTTP 之前），因此可以分离。这里起一个**自签证书**
//! 的 TLS 端点，客户端用平台验证器连接时必然在握手阶段被拒；若实现把证书错误并进
//! 「连不上」，这条用例会当场失败。

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread;

use rustls::ServerConfig;

/// 起一个只应答一次的自签证书 TLS 服务，返回它的根地址。
fn spawn_self_signed_tls_server() -> String {
    // 服务端配置与客户端验证器共用同一个进程级 crypto provider，先装一次。
    axon_rustls_provider::ensure_rustls_crypto_provider();
    let listener = TcpListener::bind("127.0.0.1:0").expect("回环端口应可绑定");
    let address = listener.local_addr().expect("应能取回真实端口");
    let certified =
        rcgen::generate_simple_self_signed(vec!["127.0.0.1".to_owned()]).expect("自签证书应可生成");
    let cert_der = certified.cert.der().clone();
    let key_der = certified.signing_key.serialize_der();
    let pkcs8 = rustls::pki_types::PrivatePkcs8KeyDer::from(key_der);
    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], pkcs8.into())
        .expect("服务端 TLS 配置应成立");

    thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let Ok(connection) = rustls::ServerConnection::new(Arc::new(config)) else {
            return;
        };
        let mut stream = rustls::StreamOwned::new(connection, stream);
        // 客户端会因为证书不受信而发告警并断开，这里的读写失败属于预期行为。
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer);
        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n");
        let _ = stream.flush();
    });
    format!("https://{address}")
}

#[test]
fn self_signed_endpoint_is_reported_as_a_certificate_problem() {
    let base = spawn_self_signed_tls_server();
    let outcome = super::probe_base_with_token(&base, None);
    match outcome {
        super::ProbeOutcome::Failed(super::ProbeFailure::CertificateInvalid(reason)) => {
            assert!(!reason.is_empty(), "必须给出可展示的中文原因");
        }
        other => panic!("自签证书必须归为「证书有问题」，实际：{other:?}"),
    }
}

#[test]
fn plaintext_client_against_a_tls_endpoint_is_not_reported_as_unreachable() {
    // 同一个自签端点，这次连都不该被当成「DNS 失败或连接被拒」。
    let base = spawn_self_signed_tls_server();
    let outcome = super::probe_base_with_token(&base, None);
    let message = match outcome {
        super::ProbeOutcome::Failed(failure) => failure.message(),
        other => panic!("应归为失败，实际：{other:?}"),
    };
    assert!(message.contains("证书"), "{message}");
}
