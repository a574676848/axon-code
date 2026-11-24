//! 模型发现路由测试：回执形状、凭据取自配置相邻 `.env`、上游失败分类、
//! 入参校验、方法边界与未命中回落。
//!
//! 上游一律用本机回环上的真实一次性端点，不打桩、不拦截 transport。

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{DISCOVER_PATH, ProvidersHttpAdapter};

/// 等待 fixture 端点收到连接的上限。
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// 构造一个指向临时配置路径的适配器。
fn fixture_adapter() -> (axon_test_support::TestTempDir, ProvidersHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = ProvidersHttpAdapter::new(directory.path().join("config.toml"));
    (directory, adapter)
}

/// 起一个只应答一次的本地端点，返回基地址与「收到的请求原文」句柄。
fn fixture_server(
    status_line: &'static str,
    body: &'static str,
) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fixture 端点应可绑定回环端口");
    let address = listener.local_addr().expect("fixture 端点应可读取本地地址");
    let handle = std::thread::spawn(move || {
        let mut stream = accept_connection(&listener);
        let request = read_request(&mut stream);
        let response = format!(
            "{status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("fixture 响应应可写出");
        request
    });
    (format!("http://{address}"), handle)
}

fn accept_connection(listener: &TcpListener) -> TcpStream {
    listener
        .set_nonblocking(true)
        .expect("fixture 端点应可设置 nonblocking");
    let started = Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("fixture 连接应恢复 blocking read");
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < FIXTURE_TIMEOUT,
                    "fixture 端点未在超时内收到请求"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fixture 端点接收连接失败：{error}"),
        }
    }
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(size) => {
                buffer.extend_from_slice(&chunk[..size]);
                if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(error) => panic!("fixture 端点读取请求失败：{error}"),
        }
    }
    String::from_utf8_lossy(&buffer).to_string()
}

/// 发一次请求并按 JSON 解析回执。
fn call(
    adapter: &ProvidersHttpAdapter,
    method: &str,
    path: &str,
    body: &[u8],
) -> Option<(u16, Value)> {
    adapter
        .handle_request(method, path, None, body)
        .map(|response| {
            let payload =
                serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
            (response.status, payload)
        })
}

#[test]
fn discover_returns_200_with_the_desktop_discover_models_shape() {
    let (_directory, adapter) = fixture_adapter();
    let (base, handle) = fixture_server(
        "HTTP/1.1 200 OK",
        r#"{"data":[{"id":"model-b"},{"id":"model-a","display_name":"模型 A","context_window":32000}]}"#,
    );

    let body = json!({
        "baseUrl": format!("{base}/v1/"),
        "protocol": "openai-chatcompletions",
    })
    .to_string();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, body.as_bytes()).expect("发现路由必须命中");

    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["endpoint", "models"], "字段必须与桌面端逐字一致");
    assert_eq!(
        payload["endpoint"],
        format!("{base}/v1/models"),
        "基地址末尾的斜杠必须被忽略"
    );

    let models = payload["models"].as_array().expect("候选模型是数组");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0]["id"], "model-a", "候选必须按标识升序");
    assert_eq!(models[0]["name"], "模型 A");
    assert_eq!(models[0]["context_window"], 32000);
    assert_eq!(models[0]["max_output_tokens"], Value::Null);
    assert_eq!(models[1]["id"], "model-b");
    assert_eq!(models[1]["name"], Value::Null);

    let request = handle.join().expect("fixture 端点应正常收尾");
    let request_head = request.to_ascii_lowercase();
    assert!(request_head.starts_with("get /v1/models"), "{request}");
    assert!(
        request_head.contains("accept: application/json"),
        "必须声明接受 JSON：{request}"
    );
}

#[test]
fn discover_reads_the_credential_from_the_config_adjacent_env_file() {
    let (directory, adapter) = fixture_adapter();
    let (base, handle) = fixture_server("HTTP/1.1 200 OK", r#"{"models":[{"id":"model-a"}]}"#);
    let env_name = format!("AXON_TEST_DISCOVER_KEY_{}", std::process::id());

    let mut config = axon_config::AxonConfig::default();
    let mut provider = axon_config::ProviderConfig::new(
        "fixture",
        axon_config::PROVIDER_PROTOCOL_ANTHROPIC,
        format!("{base}/v1"),
    );
    provider.api_key_env = Some(env_name.clone());
    config.providers.push(provider);
    axon_config::ConfigLoader::save_to_path(&directory.path().join("config.toml"), &config)
        .expect("测试配置必须可写");
    std::fs::write(
        directory.path().join(".env"),
        format!("{env_name}=fixture-secret\n"),
    )
    .expect("测试 .env 必须可写");

    let body = json!({ "provider": "fixture" }).to_string();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, body.as_bytes()).expect("发现路由必须命中");
    assert_eq!(status, 200, "{payload}");
    assert_eq!(payload["models"].as_array().map(Vec::len), Some(1));

    let request = handle.join().expect("fixture 端点应正常收尾");
    let request_head = request.to_ascii_lowercase();
    assert!(
        request_head.contains("x-api-key: fixture-secret"),
        "Anthropic 协议必须使用 x-api-key 头：{request}"
    );
    assert!(
        request_head.contains("anthropic-version: 2023-06-01"),
        "Anthropic 协议必须带版本头：{request}"
    );
}

#[test]
fn discover_rejects_an_unregistered_provider_without_a_base_url() {
    let (_directory, adapter) = fixture_adapter();
    let body = json!({ "provider": "未登记的供应商" }).to_string();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, body.as_bytes()).expect("发现路由必须命中");

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_request");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("未登记的供应商")),
        "{payload}"
    );
}

#[test]
fn discover_rejects_a_base_url_without_a_scheme() {
    let (_directory, adapter) = fixture_adapter();
    let body = json!({ "baseUrl": "api.example.com/v1" }).to_string();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, body.as_bytes()).expect("发现路由必须命中");

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_request");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("http://"))
    );
}

#[test]
fn discover_reports_an_upstream_error_as_502() {
    let (_directory, adapter) = fixture_adapter();
    let (base, handle) = fixture_server("HTTP/1.1 401 Unauthorized", r#"{"error":"denied"}"#);

    let body = json!({ "baseUrl": format!("{base}/v1") }).to_string();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, body.as_bytes()).expect("发现路由必须命中");

    assert_eq!(status, 502);
    assert_eq!(payload["error"], "discover_failed");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("401")),
        "必须如实报告上游状态码：{payload}"
    );
    handle.join().expect("fixture 端点应正常收尾");
}

#[test]
fn discover_reports_a_non_json_upstream_body_as_502() {
    let (_directory, adapter) = fixture_adapter();
    let (base, handle) = fixture_server("HTTP/1.1 200 OK", "这不是 JSON");

    let body = json!({ "baseUrl": format!("{base}/v1") }).to_string();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, body.as_bytes()).expect("发现路由必须命中");

    assert_eq!(status, 502);
    assert_eq!(payload["error"], "discover_failed");
    handle.join().expect("fixture 端点应正常收尾");
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture_adapter();
    for path in [
        "/api/v1/provider",
        "/api/v1/providers",
        "/api/v1/providers/discover/extra",
        "/api/v1/config",
    ] {
        assert!(
            adapter.handle_request("POST", path, None, &[]).is_none(),
            "{path} 不属于发现族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture_adapter();
    for method in ["GET", "PUT", "DELETE"] {
        let (status, payload) =
            call(&adapter, method, DISCOVER_PATH, &[]).expect("发现路由必须命中");
        assert_eq!(status, 405, "{method}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}

#[test]
fn body_that_is_not_json_returns_invalid_json() {
    let (_directory, adapter) = fixture_adapter();
    let (status, payload) =
        call(&adapter, "POST", DISCOVER_PATH, b"provider=fixture").expect("发现路由必须命中");

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_json");
}
