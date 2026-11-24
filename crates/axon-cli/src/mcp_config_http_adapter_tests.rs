//! MCP 配置路由测试：读写同构、环境变量值不外泄、JSON 校验、方法边界与失败分支。

use axon_daemon::GatewayHttpRouteHandler;
use serde_json::{Value, json};

use super::{MCP_CONFIG_FILE, MCP_CONFIG_PATH, McpConfigHttpAdapter};

/// 构造一个指向临时运行时根的适配器。
fn fixture() -> (axon_test_support::TestTempDir, McpConfigHttpAdapter) {
    let directory = axon_test_support::test_temp_dir();
    let adapter = McpConfigHttpAdapter::new(directory.path().to_path_buf());
    (directory, adapter)
}

/// 发一次请求并按 JSON 解析回执。
fn call(adapter: &McpConfigHttpAdapter, method: &str, path: &str, body: &[u8]) -> (u16, Value) {
    let response = adapter
        .handle_request(method, path, None, body)
        .expect("MCP 配置路由必须命中");
    let payload = serde_json::from_slice::<Value>(&response.body).expect("正文必须是合法 JSON");
    (response.status, payload)
}

/// 一份合法的 `.mcp.json` 全文：含密钥值，用于验证投影不外泄。
const SAMPLE_RAW: &str = r#"{
  "mcpServers": {
    "stdio-demo": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "demo-pkg"],
      "env": { "DEMO_TOKEN": "sk-should-never-leak" }
    },
    "http-demo": {
      "type": "http",
      "url": "https://mcp.example.com/stream"
    }
  }
}
"#;

#[test]
fn get_reports_an_absent_file_as_empty_raw_and_empty_list() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "GET", MCP_CONFIG_PATH, &[]);

    assert_eq!(status, 200, "文件不存在不是错误");
    assert_eq!(payload, json!({ "raw": "", "servers": [] }));
}

#[test]
fn get_projects_servers_without_leaking_env_values() {
    let (directory, adapter) = fixture();
    std::fs::write(directory.path().join(MCP_CONFIG_FILE), SAMPLE_RAW)
        .expect("测试 .mcp.json 必须可写");

    let (status, payload) = call(&adapter, "GET", MCP_CONFIG_PATH, &[]);
    assert_eq!(status, 200);
    let mut keys = payload
        .as_object()
        .expect("载荷必须是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    assert_eq!(keys, vec!["raw", "servers"], "载荷只允许这两项");
    assert_eq!(payload["raw"], SAMPLE_RAW, "原文必须逐字回传");

    let servers = payload["servers"].as_array().expect("服务器清单是数组");
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["name"], "http-demo", "清单必须按名称升序");
    assert_eq!(servers[0]["kind"], "http");
    assert_eq!(servers[0]["url"], "https://mcp.example.com/stream");
    assert_eq!(servers[0]["command"], "");
    assert_eq!(servers[0]["args"], json!([]));
    assert_eq!(servers[0]["env_keys"], json!([]));

    assert_eq!(servers[1]["name"], "stdio-demo");
    assert_eq!(servers[1]["kind"], "stdio");
    assert_eq!(servers[1]["command"], "npx");
    assert_eq!(servers[1]["args"], json!(["-y", "demo-pkg"]));
    assert_eq!(servers[1]["env_keys"], json!(["DEMO_TOKEN"]));

    let mut server_keys = servers[1]
        .as_object()
        .expect("服务器条目是对象")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    server_keys.sort_unstable();
    assert_eq!(
        server_keys,
        vec!["args", "command", "env_keys", "kind", "name", "url"],
        "字段必须与桌面端 McpServerDto 逐字一致"
    );

    // 服务器清单只透出变量名；`raw` 按合同要求是文件全文的忠实镜像，
    // 因此泄密面由鉴权把关，这一条只约束投影。
    let servers_json = payload["servers"].to_string();
    assert!(
        !servers_json.contains("sk-should-never-leak"),
        "服务器投影绝不能携带环境变量值：{servers_json}"
    );
}

#[test]
fn put_writes_the_raw_text_and_returns_the_same_shape_as_get() {
    let (directory, adapter) = fixture();
    let body = json!({ "raw": SAMPLE_RAW }).to_string();
    let (status, payload) = call(&adapter, "PUT", MCP_CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 200);
    assert_eq!(payload["raw"], SAMPLE_RAW);
    assert_eq!(payload["servers"].as_array().map(Vec::len), Some(2));

    let written = std::fs::read_to_string(directory.path().join(MCP_CONFIG_FILE))
        .expect(".mcp.json 必须已落盘");
    assert_eq!(written, SAMPLE_RAW, "必须原样落盘，不做字段级改写");

    let (status, reread) = call(&adapter, "GET", MCP_CONFIG_PATH, &[]);
    assert_eq!(status, 200);
    assert_eq!(reread, payload, "写入回执与随后的读取必须同构");
}

#[test]
fn put_rejects_text_that_is_not_json() {
    let (directory, adapter) = fixture();
    let body = json!({ "raw": "这不是 JSON" }).to_string();
    let (status, payload) = call(&adapter, "PUT", MCP_CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_config");
    assert!(
        !directory.path().join(MCP_CONFIG_FILE).exists(),
        "校验不通过时绝不能落盘"
    );
}

#[test]
fn put_rejects_a_document_without_the_servers_field() {
    let (_directory, adapter) = fixture();
    let body = json!({ "raw": "{\"other\":{}}" }).to_string();
    let (status, payload) = call(&adapter, "PUT", MCP_CONFIG_PATH, body.as_bytes());

    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_config");
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| message.contains("mcpServers"))
    );
}

#[test]
fn put_rejects_an_empty_document_and_a_missing_raw_field() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "PUT", MCP_CONFIG_PATH, br#"{"raw":"   "}"#);
    assert_eq!(status, 400);
    assert_eq!(payload["error"], "invalid_config");

    let (status, payload) = call(&adapter, "PUT", MCP_CONFIG_PATH, br#"{"name":"x"}"#);
    assert_eq!(status, 400);
    assert_eq!(payload["error"], "missing_raw");
}

#[test]
fn body_that_is_not_json_returns_invalid_json() {
    let (_directory, adapter) = fixture();
    let (status, payload) = call(&adapter, "PUT", MCP_CONFIG_PATH, b"raw={}");

    assert_eq!(status, 400);
    assert_eq!(payload, json!({ "error": "invalid_json" }));
}

#[test]
fn get_reports_a_broken_file_as_a_server_side_failure() {
    let (directory, adapter) = fixture();
    std::fs::write(directory.path().join(MCP_CONFIG_FILE), "这不是 JSON")
        .expect("测试文件必须可写");

    let (status, payload) = call(&adapter, "GET", MCP_CONFIG_PATH, &[]);
    assert_eq!(status, 500);
    assert_eq!(payload["error"], "invalid_config");
    assert!(payload["message"].is_string());
}

#[test]
fn unmatched_paths_are_not_swallowed() {
    let (_directory, adapter) = fixture();
    for path in [
        "/api/v1/mcp",
        "/api/v1/mcp/status",
        "/api/v1/mcp/config/extra",
        "/api/v1/config",
    ] {
        assert!(
            adapter.handle_request("GET", path, None, &[]).is_none(),
            "{path} 不属于 MCP 配置族，必须交回组合器"
        );
    }
}

#[test]
fn method_mismatch_returns_405_method_not_allowed() {
    let (_directory, adapter) = fixture();
    for method in ["POST", "DELETE", "PATCH"] {
        let (status, payload) = call(&adapter, method, MCP_CONFIG_PATH, &[]);
        assert_eq!(status, 405, "{method}");
        assert_eq!(payload, json!({ "error": "method_not_allowed" }));
    }
}
