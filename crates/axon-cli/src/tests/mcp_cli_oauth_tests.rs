use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use serde_json::Value;

use super::{AxonCli, run_parsed_cli};

#[test]
fn mcp_oauth_login_and_logout_without_leaking_tokens() {
    let root = test_root("oauth");
    let oauth_server = spawn_oauth_metadata_server();
    seed_streamable_http_oauth_config(&root, "team-tools", &oauth_server.mcp_url);
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let login_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "login",
        "team_tools",
        "--scope",
        "tools.read",
        "--client-id",
        "axon-test-client",
    ]);
    let login_result = run_parsed_cli(&login_cli);
    let login_body: Value =
        serde_json::from_str(&login_result.stdout).expect("mcp login 应输出 JSON");

    assert_eq!(login_result.exit_code, 0);
    assert_eq!(login_body["status"], "mcp_oauth_login_pending");
    assert_eq!(login_body["credentialStatus"], "oauth_pending");
    assert_eq!(login_body["tokenValueExposed"], false);
    assert_eq!(login_body["remoteTriggerAllowed"], false);
    assert_eq!(login_body["scopes"], serde_json::json!(["tools.read"]));
    assert_eq!(login_body["redirectUri"], "http://127.0.0.1/oauth/callback");
    let authorization_url = login_body["authorizationUrl"]
        .as_str()
        .expect("authorizationUrl 应存在");
    assert!(authorization_url.starts_with(&oauth_server.authorize_url));
    assert!(authorization_url.contains("response_type=code"));
    assert!(authorization_url.contains("client_id=axon-test-client"));
    assert!(authorization_url.contains("code_challenge="));
    assert!(authorization_url.contains("code_challenge_method=S256"));
    assert!(authorization_url.contains("resource="));
    assert!(authorization_url.contains("scope=tools.read"));
    assert!(!login_result.stdout.contains("secret-access-token"));

    let logout_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "logout",
        "team_tools",
    ]);
    let logout_result = run_parsed_cli(&logout_cli);
    let logout_body: Value =
        serde_json::from_str(&logout_result.stdout).expect("mcp logout 应输出 JSON");
    assert_eq!(logout_result.exit_code, 0);
    assert_eq!(logout_body["status"], "mcp_oauth_logged_out");
    assert_eq!(logout_body["tokenValueExposed"], false);
    assert!(!logout_result.stdout.contains("secret-access-token"));
    assert!(oauth_server.saw_metadata_request());
    let _ = fs::remove_dir_all(root);
}

fn seed_streamable_http_oauth_config(root: &Path, server_id: &str, url: &str) {
    fs::create_dir_all(root).expect("测试 Axon 根目录应可创建");
    fs::write(
        root.join(".mcp.json"),
        serde_json::json!({
            "mcpServers": {
                (server_id): {
                    "type": "streamable_http",
                    "url": url,
                }
            }
        })
        .to_string(),
    )
    .expect("OAuth Streamable HTTP MCP 配置应可写入");
}

struct OAuthMetadataServer {
    mcp_url: String,
    authorize_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl OAuthMetadataServer {
    fn saw_metadata_request(&self) -> bool {
        self.requests
            .lock()
            .expect("请求记录锁应可获取")
            .iter()
            .any(|request| request.starts_with("GET /.well-known/oauth-protected-resource"))
    }
}

fn spawn_oauth_metadata_server() -> OAuthMetadataServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("OAuth metadata server 应可绑定端口");
    let address = listener
        .local_addr()
        .expect("OAuth metadata server 地址应可读取");
    let origin = format!("http://{address}");
    let authorize_url = format!("{origin}/oauth/authorize");
    let protected_resource_metadata = format!(
        "{{\"resource\":\"{origin}/mcp\",\"authorization_servers\":[\"{origin}/oauth\"],\"scopes_supported\":[\"tools.read\",\"tools.write\"]}}"
    );
    let authorization_server_metadata = format!(
        "{{\"issuer\":\"{origin}/oauth\",\"authorization_endpoint\":\"{origin}/oauth/authorize\",\"token_endpoint\":\"{origin}/oauth/token\"}}"
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_for_server = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming().take(16) {
            let Ok(mut stream) = stream else {
                continue;
            };
            let mut buffer = [0_u8; 4096];
            let bytes = stream.read(&mut buffer).unwrap_or_default();
            let request = String::from_utf8_lossy(&buffer[..bytes]).to_string();
            requests_for_server
                .lock()
                .expect("请求记录锁应可获取")
                .push(request.clone());
            let body = if request.starts_with("GET /.well-known/oauth-protected-resource") {
                protected_resource_metadata.clone()
            } else if request.starts_with("GET /.well-known/oauth-authorization-server/oauth") {
                authorization_server_metadata.clone()
            } else {
                "{}".to_owned()
            };
            write_json_response(&mut stream, &body);
        }
    });
    OAuthMetadataServer {
        mcp_url: format!("{origin}/mcp"),
        authorize_url,
        requests,
    }
}

fn write_json_response(stream: &mut std::net::TcpStream, body: &str) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nMcp-Session-Id: test-session\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn test_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root().join(format!(
        "axon-mcp-cli-oauth-{name}-{}-{:?}-{nonce}",
        std::process::id(),
        std::thread::current().id()
    ))
}
