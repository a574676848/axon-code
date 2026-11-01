//! 文档路由测试：读写同构、缺失文件回空串、落盘文件名、空正文拒绝、
//! 方法边界与未命中回落。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{AGENTS_PATH, DocumentsHttpAdapter, SOUL_PATH};

/// 构造一个指向临时运行时根的适配器。
fn fixture() -> (axon_test_support::TestTempDir, DocumentsHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = DocumentsHttpAdapter::new(directory.path().to_path_buf());
    (directory, adapter)
}

/// 发一次请求并按 JSON 解析回执。
fn call(adapter: &DocumentsHttpAdapter, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    let response = adapter
        .handle_request(method, path, None, body)
        .expect("文档路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
}

#[test]
fn absent_documents_are_reported_as_empty_text_not_404() {
    let (_directory, adapter) = fixture();
    for path in [SOUL_PATH, AGENTS_PATH] {
        let (status, payload) = call(&adapter, "GET", path, &[]);
        assert_eq!(status, 200, "{path}");
        assert_eq!(payload, json!({ "text": "" }), "{path}");
    }
}

#[test]
fn get_returns_the_document_text_without_a_bom() {
    let (directory, adapter) = fixture();
    std::fs::write(
        directory.path().join("AGENTS.md"),
        "\u{feff}# 行为约束\n\n中文正文\n",
    )
    .expect("测试文档必须可写");

    let (status, payload) = call(&adapter, "GET", AGENTS_PATH, &[]);
    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["text"], "载荷只允许 text 一项");
    assert_eq!(payload["text"], "# 行为约束\n\n中文正文\n");
}

#[test]
fn put_writes_the_document_and_returns_the_same_shape_as_get() {
    let (directory, adapter) = fixture();
    for (path, file_name, text) in [
        (SOUL_PATH, "SOUL.md", "# 人设\n\n冷静、克制。\n"),
        (AGENTS_PATH, "AGENTS.md", "# 行为\n\n先读后改。\n"),
    ] {
        let body = json!({ "text": text }).to_string();
        let (status, payload) = call(&adapter, "PUT", path, body.as_bytes());
        assert_eq!(status, 200, "{path}");
        assert_eq!(payload, json!({ "text": text }), "{path}");

        let written =
            std::fs::read_to_string(directory.path().join(file_name)).expect("文档必须已落盘");
        assert_eq!(written, text, "{file_name} 必须逐字落盘");

        let (status, reread) = call(&adapter, "GET", path, &[]);
        assert_eq!(status, 200, "{path}");
        assert_eq!(reread, payload, "{path} 写入回执与随后的读取必须同构");
    }
}

#[test]
fn put_rejects_an_empty_document() {
    let (directory, adapter) = fixture();
    for text in ["", "   \n\t"] {
        let body = json!({ "text": text }).to_string();
        let (status, payload) = call(&adapter, "PUT", SOUL_PATH, body.as_bytes());
        assert_eq!(status, 400, "{text:?}");
        assert_eq!(payload["error"], "empty_text");
    }
    assert!(
        !directory.path().join("SOUL.md").exists(),
        "拒绝的请求绝不能落盘"
    );
}

#[test]
fn put_reports_a_missing_text_field_and_a_non_json_body() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "PUT", SOUL_PATH, br#"{"content":"x"}"#);
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "missing_text" }));

    let (status, payload) = call(&adapter, "PUT", AGENTS_PATH, b"text=abc");
    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "invalid_json" }));
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/documents",
        "/api/v1/documents/other",
        "/api/v1/documents/soul/extra",
        "/api/v1/config",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于文档族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture();
    for (method, path) in [
        ("POST", SOUL_PATH),
        ("DELETE", SOUL_PATH),
        ("PATCH", AGENTS_PATH),
    ] {
        let (status, payload) = call(&adapter, method, path, &[]);
        assert_eq!(status, 405, "{method} {path}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}
