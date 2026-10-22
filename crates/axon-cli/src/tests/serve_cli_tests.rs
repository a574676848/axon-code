use std::{
    fs,
    io::{Read as _, Write as _},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    thread,
    time::Duration,
};

use serde_json::Value;

use super::{AxonCli, run_parsed_cli};
use axon_session::{JsonlThreadEventStore, ThreadEventRecord, ThreadEventStore};

const SERVE_FIXTURE_STARTUP_TIMEOUT: Duration = Duration::from_millis(500);
const SERVE_FIXTURE_RETRY_INTERVAL: Duration = Duration::from_millis(20);

#[test]
fn serve_health_starts_loopback_http_without_remote_execution() {
    let port = free_port();
    let handle = thread::spawn(move || {
        let cli = AxonCli::parse_from_args([
            "axon",
            "--format",
            "json",
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--max-requests",
            "1",
        ]);
        run_parsed_cli(&cli)
    });

    let response = request_with_retry(port, "GET /v1/health HTTP/1.1\r\nhost: localhost\r\n\r\n");
    let result = handle.join().expect("serve 线程应正常退出");
    let report: Value = serde_json::from_str(&result.stdout).expect("serve report 应为 JSON");
    let body = response_body_json(&response);

    assert_eq!(result.exit_code, 0);
    assert_eq!(report["status"], "serve_stopped");
    assert_eq!(report["handledRequests"], 1);
    assert_eq!(report["remoteTriggerAllowed"], false);
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(body["status"], "ok");
    assert_eq!(body["service"], "axon-serve");
    assert_eq!(body["remoteTriggerAllowed"], false);
    assert_eq!(body["remoteStdinAllowed"], false);
}

#[test]
fn serve_requires_auth_for_models_when_token_file_is_configured() {
    let root = test_root("auth");
    fs::create_dir_all(&root).expect("测试目录应可创建");
    let token_path = root.join("serve-token.txt");
    fs::write(&token_path, "0123456789abcdef").expect("token 文件应可写入");
    let port = free_port();
    let token_arg = token_path.to_string_lossy().to_string();
    let handle = thread::spawn(move || {
        let cli = AxonCli::parse_from_args([
            "axon",
            "--format",
            "json",
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--auth-token-file",
            &token_arg,
            "--max-requests",
            "2",
        ]);
        run_parsed_cli(&cli)
    });

    let unauthorized =
        request_with_retry(port, "GET /v1/models HTTP/1.1\r\nhost: localhost\r\n\r\n");
    let authorized = request_with_retry(
        port,
        "GET /v1/models HTTP/1.1\r\nhost: localhost\r\nauthorization: Bearer 0123456789abcdef\r\n\r\n",
    );
    let result = handle.join().expect("serve 线程应正常退出");

    assert_eq!(result.exit_code, 0);
    assert!(unauthorized.starts_with("HTTP/1.1 401 Unauthorized"));
    assert!(authorized.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(
        response_body_json(&authorized)["providerRawPayloadExposed"],
        false
    );
    assert!(!authorized.contains("0123456789abcdef"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn serve_sessions_lists_ref_only_run_sessions_without_raw_transcript() {
    let root = test_root("sessions");
    fs::create_dir_all(&root).expect("测试根目录应可创建");
    let config_path = root.join("config.toml");
    fs::write(
        &config_path,
        "[agent]\ndefault_model = \"balanced\"\n",
    )
    .expect("测试配置应可写入");
    let events = concat!(
            "{\"schemaVersion\":3,\"threadId\":\"session-old\",\"sequence\":0,\"recordedAtMs\":10,\"event\":{\"type\":\"threadStarted\",\"session_root_id\":\"session-old\",\"parent_thread_id\":null,\"forked_from_thread_id\":null,\"source\":\"cliRun\",\"ephemeral\":false,\"settings\":{\"cwd\":\"E:\\\\fixture\",\"provider\":\"fixture\",\"model\":\"small\",\"selectedSkillName\":null}}}\n",
            "{\"schemaVersion\":3,\"threadId\":\"session-old\",\"sequence\":1,\"recordedAtMs\":11,\"event\":{\"type\":\"turnCompleted\",\"turn\":{\"turnId\":\"session-old-turn-0\",\"sequence\":0,\"status\":\"completed\",\"historyItems\":[{\"type\":\"userMessage\",\"sequence\":0,\"part_id\":\"session-old-user-0\",\"content\":\"旧提示\",\"local_images\":[],\"local_files\":[],\"provider_metadata\":null},{\"type\":\"assistantMessage\",\"sequence\":1,\"part_id\":\"session-old-assistant-0\",\"content\":\"旧回答\",\"provider_metadata\":null}],\"tokenUsageJson\":null,\"startedAtMs\":11,\"completedAtMs\":11,\"interruptedAtMs\":null}}}\n",
            "{\"schemaVersion\":3,\"threadId\":\"session-new\",\"sequence\":2,\"recordedAtMs\":20,\"event\":{\"type\":\"threadStarted\",\"session_root_id\":\"session-new\",\"parent_thread_id\":\"session-old\",\"forked_from_thread_id\":\"session-old\",\"source\":\"cliRun\",\"ephemeral\":false,\"settings\":{\"cwd\":\"E:\\\\fixture\",\"provider\":\"fixture\",\"model\":\"large\",\"selectedSkillName\":null}}}\n",
            "{\"schemaVersion\":3,\"threadId\":\"session-new\",\"sequence\":3,\"recordedAtMs\":21,\"event\":{\"type\":\"turnCompleted\",\"turn\":{\"turnId\":\"session-new-turn-0\",\"sequence\":0,\"status\":\"completed\",\"historyItems\":[{\"type\":\"userMessage\",\"sequence\":0,\"part_id\":\"session-new-user-0\",\"content\":\"敏感提示\",\"local_images\":[],\"local_files\":[],\"provider_metadata\":null},{\"type\":\"assistantMessage\",\"sequence\":1,\"part_id\":\"session-new-assistant-0\",\"content\":\"敏感回答\",\"provider_metadata\":null}],\"tokenUsageJson\":null,\"startedAtMs\":21,\"completedAtMs\":21,\"interruptedAtMs\":null}}}\n",
        );
    let store = JsonlThreadEventStore;
    for line in events.lines() {
        let record = serde_json::from_str::<ThreadEventRecord>(line)
            .expect("session fixture 事件应有效");
        store
            .append(&root, &record)
            .expect("session fixture 事件应可写入");
    }
    let port = free_port();
    let config_arg = config_path.to_string_lossy().to_string();
    let handle = thread::spawn(move || {
        let cli = AxonCli::parse_from_args([
            "axon",
            "--config",
            &config_arg,
            "--format",
            "json",
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--max-requests",
            "1",
        ]);
        run_parsed_cli(&cli)
    });

    let response = request_with_retry(
        port,
        "GET /v1/sessions?limit=1 HTTP/1.1\r\nhost: localhost\r\n\r\n",
    );
    let result = handle.join().expect("serve 线程应正常退出");
    let body = response_body_json(&response);

    assert_eq!(result.exit_code, 0);
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert_eq!(body["status"], "session_ready");
    assert_eq!(body["limit"], 1);
    assert_eq!(body["totalRecords"], 2);
    assert_eq!(
        body["sessions"]
            .as_array()
            .expect("sessions 应为数组")
            .len(),
        1
    );
    assert_eq!(body["sessions"][0]["threadId"], "session-new");
    assert_eq!(body["sessions"][0]["parentThreadId"], "session-old");
    assert_eq!(body["sessions"][0]["turnCount"], 1);
    assert_eq!(body["sessionRuntimePending"], false);
    assert_eq!(body["postSessionAllowed"], false);
    assert_eq!(body["sseAllowed"], false);
    assert_eq!(body["remoteTriggerAllowed"], false);
    assert_eq!(body["rawTranscriptExposed"], false);
    assert_eq!(body["providerRawPayloadExposed"], false);
    assert!(!response.contains("敏感提示"));
    assert!(!response.contains("敏感回答"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn serve_rejects_public_bind_without_auth_token() {
    let cli = AxonCli::parse_from_args([
        "axon",
        "--format",
        "json",
        "serve",
        "--host",
        "0.0.0.0",
        "--port",
        "0",
        "--max-requests",
        "1",
    ]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 2);
    assert!(result.stderr.contains("auth-token-file"));
}

fn request_with_retry(port: u16, request: &str) -> String {
    let started = std::time::Instant::now();
    let mut last_error = None;
    while started.elapsed() < SERVE_FIXTURE_STARTUP_TIMEOUT {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                stream.write_all(request.as_bytes()).expect("请求应可写入");
                let mut response = String::new();
                stream.read_to_string(&mut response).expect("响应应可读取");
                return response;
            }
            Err(error) => {
                last_error = Some(error);
                thread::sleep(SERVE_FIXTURE_RETRY_INTERVAL);
            }
        }
    }
    panic!("无法在 {SERVE_FIXTURE_STARTUP_TIMEOUT:?} 内连接 serve fixture: {last_error:?}");
}

fn response_body_json(response: &str) -> Value {
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .expect("HTTP 响应应包含 body");
    serde_json::from_str(body).expect("HTTP body 应为 JSON")
}

fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("测试端口应可绑定")
        .local_addr()
        .expect("测试端口应可读取")
        .port()
}

fn test_root(name: &str) -> PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-serve-cli-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}
