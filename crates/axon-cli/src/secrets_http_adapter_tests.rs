//! 密钥路由测试：状态查询、写入与清除、明文不出现在任何回执里、方法边界与失败分支。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{SECRETS_PATH, SECRETS_STATUS_PATH, SecretsHttpAdapter};

/// 构造一个指向临时配置路径的适配器。
fn fixture() -> (axon_test_support::TestTempDir, SecretsHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = SecretsHttpAdapter::new(directory.path().join("config.toml"));
    (directory, adapter)
}

/// 生成一个只在本进程内唯一的变量名，避免并行测试互相干扰。
fn secret_name(tag: &str) -> String {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("AXON_TEST_SECRET_{tag}_{}_{stamp}", std::process::id())
}

/// 发一次请求并取回状态码与原始正文。
fn call_raw(
    adapter: &SecretsHttpAdapter,
    method: &str,
    path: &str,
    query: Option<&str>,
    body: &[u8],
) -> (u16, String) {
    let response = adapter
        .handle_request(method, path, query, body)
        .expect("密钥路由必须命中");
    (
        response.status,
        String::from_utf8_lossy(&response.body).to_string(),
    )
}

/// 发一次请求并按 JSON 解析回执。
fn call(
    adapter: &SecretsHttpAdapter,
    method: &str,
    path: &str,
    query: Option<&str>,
    body: &[u8],
) -> (u16, Value) {
    let (status, raw) = call_raw(adapter, method, path, query, body);
    let payload = serde_json::from_str::<Value>(&raw).expect("正文必须是合法 JSON");
    (status, payload)
}

/// 查询某个变量名的状态。
fn status_of(adapter: &SecretsHttpAdapter, name: &str) -> bool {
    let query = format!("name={name}");
    let (status, payload) = call(adapter, "GET", SECRETS_STATUS_PATH, Some(&query), &[]);
    assert_eq!(status, 200);
    assert_eq!(payload["name"], name);
    payload["configured"]
        .as_bool()
        .expect("configured 必须是布尔值")
}

#[test]
fn status_route_reports_missing_and_present_credentials() {
    let (directory, adapter) = fixture();
    let name = secret_name("STATUS");

    assert!(!status_of(&adapter, &name), "没有 .env 时必须报未配置");

    let body = json!({ "name": name, "value": "fixture-plain-secret" }).to_string();
    let (status, payload) = call(&adapter, "PUT", SECRETS_PATH, None, body.as_bytes());
    assert_eq!(status, 200);
    assert_eq!(payload["configured"], true);

    assert!(status_of(&adapter, &name), "写入之后必须报已配置");

    let written = std::fs::read_to_string(directory.path().join(".env")).expect(".env 必须已落盘");
    assert!(written.contains(&name), "{written}");
    assert!(written.contains("fixture-plain-secret"), "{written}");
}

#[test]
fn responses_never_echo_the_secret_plaintext() {
    let (_directory, adapter) = fixture();
    let name = secret_name("ECHO");
    let secret = "sk-never-echo-this-value";
    let body = json!({ "name": name, "value": secret }).to_string();

    let (status, raw) = call_raw(&adapter, "PUT", SECRETS_PATH, None, body.as_bytes());
    assert_eq!(status, 200);
    assert!(!raw.contains(secret), "写入回执绝不能带上明文：{raw}");
    let payload = serde_json::from_str::<Value>(&raw).expect("正文必须是合法 JSON");
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["configured", "name"], "回执只允许这两个字段");

    let query = format!("name={name}");
    let (status, raw) = call_raw(&adapter, "GET", SECRETS_STATUS_PATH, Some(&query), &[]);
    assert_eq!(status, 200);
    assert!(!raw.contains(secret), "状态回执同样不得带上明文：{raw}");
}

#[test]
fn writing_keeps_the_existing_env_content_and_an_empty_value_clears() {
    let (directory, adapter) = fixture();
    let env_path = directory.path().join(".env");
    std::fs::write(&env_path, "# 保留的注释\nEXISTING_KEY=existing-value\n")
        .expect("测试 .env 必须可写");

    let name = secret_name("KEEP");
    let body = json!({ "name": name, "value": "first-value" }).to_string();
    assert_eq!(
        call(&adapter, "PUT", SECRETS_PATH, None, body.as_bytes()).0,
        200
    );

    let written = std::fs::read_to_string(&env_path).expect(".env 必须存在");
    assert!(
        written.contains("# 保留的注释"),
        "既有注释必须保留：{written}"
    );
    assert!(
        written.contains("EXISTING_KEY=existing-value"),
        "既有键值必须保留：{written}"
    );
    assert!(written.contains(&name), "{written}");

    let clear = json!({ "name": name, "value": "" }).to_string();
    let (status, payload) = call(&adapter, "PUT", SECRETS_PATH, None, clear.as_bytes());
    assert_eq!(status, 200);
    assert_eq!(payload["configured"], false, "空值表示清除");
    assert!(!status_of(&adapter, &name), "清除之后必须报未配置");

    let after_clear = std::fs::read_to_string(&env_path).expect(".env 必须仍然存在");
    assert!(
        after_clear.contains("EXISTING_KEY=existing-value"),
        "清除只影响目标键：{after_clear}"
    );
    assert!(!after_clear.contains("first-value"), "{after_clear}");
}

#[test]
fn unknown_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/secret",
        "/api/v1/secrets/status/extra",
        "/api/v1/config",
        "/api/v1/skills",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于密钥族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture();
    for (method, path) in [
        ("POST", SECRETS_PATH),
        ("GET", SECRETS_PATH),
        ("PUT", SECRETS_STATUS_PATH),
        ("DELETE", SECRETS_STATUS_PATH),
    ] {
        let (status, payload) = call(&adapter, method, path, Some("name=X"), b"");
        assert_eq!(status, 405, "{method} {path}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}

#[test]
fn status_without_name_returns_missing_name() {
    let (_directory, adapter) = fixture();
    for query in [
        None,
        Some(""),
        Some("other=1"),
        Some("name="),
        Some("name=   "),
    ] {
        let (status, payload) = call(&adapter, "GET", SECRETS_STATUS_PATH, query, &[]);
        assert_eq!(status, 400, "{query:?}");
        assert_eq!(payload, json!({ "error": "missing_name" }), "{query:?}");
    }
}

#[test]
fn status_with_an_illegal_name_returns_invalid_name() {
    let (_directory, adapter) = fixture();
    for query in ["name=1BAD", "name=BAD-KEY", "name=BAD%20KEY"] {
        let (status, payload) = call(&adapter, "GET", SECRETS_STATUS_PATH, Some(query), &[]);
        assert_eq!(status, 400, "{query}");
        assert_eq!(payload["error"], "invalid_name", "{query}");
        assert!(payload["message"].is_string(), "非法变量名必须带上中文原因");
    }
}

#[test]
fn body_that_is_not_json_returns_invalid_json() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "PUT", SECRETS_PATH, None, b"name=FOO");
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "invalid_json" }));
}

#[test]
fn put_without_a_name_or_value_field_is_reported_field_by_field() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "PUT", SECRETS_PATH, None, br#"{"value":"x"}"#);
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "missing_name" }));

    let body = json!({ "name": secret_name("NOVALUE") }).to_string();
    let (status, payload) = call(&adapter, "PUT", SECRETS_PATH, None, body.as_bytes());
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "missing_value" }));
}

#[test]
fn a_value_with_a_newline_is_rejected_before_writing() {
    let (directory, adapter) = fixture();
    let body = json!({ "name": secret_name("NL"), "value": "first\nSECOND=injected" }).to_string();
    let (status, payload) = call(&adapter, "PUT", SECRETS_PATH, None, body.as_bytes());

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_value");
    assert!(
        !directory.path().join(".env").exists(),
        "拒绝的请求绝不能落盘"
    );
}
