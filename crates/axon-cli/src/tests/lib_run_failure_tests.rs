use super::lib_run_test_support::temp_config_path;
use super::{run_parsed_cli, AxonCli};

const FIXTURE_SERVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[test]
fn run_command_does_not_validate_model_configuration_in_frontend() {
    let path = temp_config_path("run-missing-model");
    let _ = std::fs::remove_file(&path);
    axon_config::ConfigLoader::save_to_path(&path, &axon_config::AxonConfig::default()).unwrap();
    let path_arg = path.to_string_lossy().to_string();
    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "run",
        "解释错误",
    ]);

    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 1);
    assert!(result.stderr.contains("连接 Daemon IPC 失败"));
    assert!(!result
        .stderr
        .contains("agent.default_model 必须引用已配置模型名"));
    assert!(result.stdout.is_empty());

    let _ = std::fs::remove_file(path);
}

#[test]
fn run_command_text_failure_reports_daemon_ipc_error() {
    let path = temp_config_path("run-daemon-failure");
    let _ = std::fs::remove_file(&path);
    axon_config::ConfigLoader::save_to_path(&path, &axon_config::AxonConfig::default()).unwrap();
    let path_arg = path.to_string_lossy().to_string();
    let cwd_arg = path
        .parent()
        .expect("测试配置应有父目录")
        .to_string_lossy()
        .to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--cwd",
        &cwd_arg,
        "--log-level",
        "debug",
        "run",
        "--timeout",
        "2",
        "解释错误",
    ]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 1);
    assert!(result.stdout.is_empty());
    assert!(result.stderr.contains("连接 Daemon IPC 失败"));
    assert!(!result.stderr.contains("provider_transport"));

    let _ = std::fs::remove_file(path);
}

#[test]
fn run_command_inherits_global_json_format() {
    let path = temp_config_path("run-global-json-format");
    let _ = std::fs::remove_file(&path);
    axon_config::ConfigLoader::save_to_path(&path, &axon_config::AxonConfig::default()).unwrap();
    let path_arg = path.to_string_lossy().to_string();
    let cwd_arg = path
        .parent()
        .expect("测试配置应有父目录")
        .to_string_lossy()
        .to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--format",
        "json",
        "--config",
        &path_arg,
        "--cwd",
        &cwd_arg,
        "run",
        "解释错误",
    ]);
    let result = run_parsed_cli(&cli);
    let body: serde_json::Value = serde_json::from_str(&result.stdout).expect("run 应输出 JSON");

    assert_eq!(result.exit_code, 1);
    assert_eq!(body["event"], "run_failed");
    assert!(body["error"]
        .as_str()
        .is_some_and(|error| error.contains("连接 Daemon IPC 失败")));
    assert!(result.stderr.is_empty());

    let _ = std::fs::remove_file(path);
}

#[test]
fn providers_test_loads_config_adjacent_env_credential() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = axon_test_support::test_temp_root().join(format!("axon-provider-test-{unique}"));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("config.toml");
    let env_name = format!("AXON_PROVIDER_TEST_KEY_{unique}");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut stream = accept_fixture_connection(&listener);
        let request = read_fixture_http_request(&mut stream).to_ascii_lowercase();
        assert!(request.contains("authorization: bearer fixture-secret"));
        let response =
            "HTTP/1.1 200 OK\r\ncontent-length: 2\r\ncontent-type: application/json\r\n\r\n{}";
        std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
    });
    let mut config = axon_config::AxonConfig::default();
    let mut provider = axon_config::ProviderConfig::new(
        "fixture",
        axon_config::PROVIDER_PROTOCOL_OPENAI,
        format!("http://{address}/v1"),
    );
    provider.api_key_env = Some(env_name.clone());
    config.providers.push(provider);
    axon_config::ConfigLoader::save_to_path(&path, &config).unwrap();
    std::fs::write(root.join(".env"), format!("{env_name}=fixture-secret\n")).unwrap();
    let path_arg = path.to_string_lossy().to_string();
    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "providers",
        "test",
        "fixture",
    ]);

    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 0);
    assert!(result.stdout.contains("credential: present"));
    assert!(result.stdout.contains("http_status: 200"));
    assert!(result.stderr.is_empty());

    handle.join().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

fn accept_fixture_connection(listener: &std::net::TcpListener) -> std::net::TcpStream {
    listener
        .set_nonblocking(true)
        .expect("fixture listener 应可设置 nonblocking");
    let started = std::time::Instant::now();
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .expect("fixture stream 应恢复 blocking read");
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    started.elapsed() < FIXTURE_SERVER_TIMEOUT,
                    "fixture provider 未在 {FIXTURE_SERVER_TIMEOUT:?} 内收到请求"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("fixture provider 接收连接失败: {error}"),
        }
    }
}

fn read_fixture_http_request(stream: &mut std::net::TcpStream) -> String {
    stream
        .set_read_timeout(Some(FIXTURE_SERVER_TIMEOUT))
        .expect("fixture stream 应可设置 read timeout");
    let mut request = String::new();
    let mut buffer = [0_u8; 1024];
    loop {
        let read = std::io::Read::read(stream, &mut buffer).unwrap();
        assert!(read != 0, "fixture provider 在请求头完成前连接关闭");
        request.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if request.contains("\r\n\r\n") {
            break;
        }
    }
    request
}
