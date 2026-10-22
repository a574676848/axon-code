use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use axon_agent_runtime::McpPoolStoreRuntime;
use axon_jsonl::ShardedJsonlStore;

use super::{AxonCli, run_parsed_cli};

#[path = "mcp_cli_config_tests.rs"]
mod mcp_cli_config_tests;
#[path = "mcp_cli_oauth_tests.rs"]
mod mcp_cli_oauth_tests;
#[path = "mcp_cli_runtime_tests.rs"]
mod mcp_cli_runtime_tests;
#[path = "mcp_cli_test_fixtures.rs"]
pub(super) mod mcp_cli_test_fixtures;

#[test]
fn mcp_list_reports_discovered_servers_and_latest_pool_status() {
    let root = test_root("list");
    seed_mcp_server(&root, "team-tools");
    seed_pool_records(&root);
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "mcp", "list", "--limit", "10",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("mcp list 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "mcp_ready");
    assert_eq!(body["totalServers"], 1);
    assert_eq!(body["servers"][0]["serverId"], "team_tools");
    assert_eq!(body["servers"][0]["doctorStatus"], "ok");
    assert_eq!(body["servers"][0]["activeSessionCount"], 2);
    assert_eq!(body["servers"][0]["remoteTriggerAllowed"], false);
    assert!(!result.stdout.contains("secret-mcp-command"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_get_reports_config_summary_without_leaking_stdio_command() {
    let root = test_root("get-stdio");
    seed_mcp_server(&root, "team-tools");
    seed_pool_records(&root);
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "get",
        "team_tools",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("mcp get 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "mcp_server_ready");
    assert_eq!(body["server"]["serverId"], "team_tools");
    assert_eq!(body["transport"], "stdio");
    assert_eq!(body["commandConfigExposed"], false);
    assert_eq!(body["rawConfigExposed"], false);
    assert_eq!(body["remoteTriggerAllowed"], false);
    assert!(!result.stdout.contains("secret-mcp-command"));
    assert!(!result.stdout.contains("hidden"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_add_writes_stdio_config_without_leaking_command_config() {
    let root = test_root("add-stdio");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "add",
        "team-tools",
        "--",
        "secret-mcp-command",
        "token",
        "hidden",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("mcp add 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "mcp_added");
    assert_eq!(body["server"]["serverId"], "team_tools");
    assert_eq!(body["configWritten"], true);
    assert_eq!(body["commandConfigExposed"], false);
    assert_eq!(body["rawConfigExposed"], false);
    assert_eq!(body["remoteTriggerAllowed"], false);
    assert!(!result.stdout.contains("secret-mcp-command"));
    assert!(!result.stdout.contains("hidden"));

    let get_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "get",
        "team_tools",
    ]);
    let get_result = run_parsed_cli(&get_cli);
    let get_body: Value = serde_json::from_str(&get_result.stdout).expect("mcp get 应输出 JSON");
    assert_eq!(get_body["transport"], "stdio");
    assert!(!get_result.stdout.contains("secret-mcp-command"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_add_writes_streamable_http_config_without_leaking_token_value() {
    let root = test_root("add-http");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "add",
        "remote-tools",
        "--url",
        "http://127.0.0.1:9/mcp",
        "--bearer-token-env-var",
        "AXON_TEST_MCP_TOKEN",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("mcp add 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "mcp_added");
    assert_eq!(body["server"]["serverId"], "remote_tools");
    assert!(!result.stdout.contains("secret-token-value"));

    let get_cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "get",
        "remote_tools",
    ]);
    let get_result = run_parsed_cli(&get_cli);
    let get_body: Value = serde_json::from_str(&get_result.stdout).expect("mcp get 应输出 JSON");
    assert_eq!(get_body["transport"], "streamable_http");
    assert_eq!(get_body["url"], "http://127.0.0.1:9/mcp");
    assert_eq!(get_body["bearerTokenEnvVar"], "AXON_TEST_MCP_TOKEN");
    assert_eq!(get_body["tokenValueExposed"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_remove_deletes_user_config_entry() {
    let root = test_root("remove");
    seed_mcp_server(&root, "team-tools");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon",
        "--config",
        &path_arg,
        "--format",
        "json",
        "mcp",
        "remove",
        "team_tools",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("mcp remove 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["status"], "mcp_removed");
    assert_eq!(body["removed"], true);

    let list_cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "mcp", "list",
    ]);
    let list_result = run_parsed_cli(&list_cli);
    let list_body: Value = serde_json::from_str(&list_result.stdout).expect("mcp list 应输出 JSON");
    assert_eq!(list_body["totalServers"], 0);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn mcp_list_rejects_invalid_limit() {
    let root = test_root("limit");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--format", "json", "mcp", "list", "--limit", "0",
    ]);
    let result = run_parsed_cli(&cli);

    assert_eq!(result.exit_code, 2);
    assert!(result.stderr.contains("mcp limit"));
    let _ = fs::remove_dir_all(root);
}

fn seed_mcp_server(root: &Path, server_id: &str) {
    write_mcp_server_config(
        root,
        server_id,
        serde_json::json!({
            "type": "stdio",
            "command": "secret-mcp-command",
            "args": ["--token", "hidden"]
        }),
    );
}

fn seed_workspace_skill(root: &Path) {
    let skill_dir = root.join(".axon").join("skills").join("release-notes");
    fs::create_dir_all(&skill_dir).expect("测试工作区 Skill 目录应可创建");
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: release-notes\ndescription: 测试发布说明 Skill\n---\n\nrelease notes skill\n",
    )
    .expect("测试工作区 Skill 应可写入");
}

fn seed_pool_records(root: &Path) {
    let content = [
        r#"{"schemaVersion":1,"serverId":"team_tools","recordedAtMs":1000,"activeSessionCount":1,"busySessionCount":1,"failureCount":1,"backoffRemainingMs":50,"doctorStatus":"degraded","doctorBadge":"degraded","terminalMainLoopContinues":true,"remoteTriggerAllowed":false}"#,
        r#"{"schemaVersion":1,"serverId":"team_tools","recordedAtMs":2000,"activeSessionCount":2,"busySessionCount":0,"failureCount":0,"backoffRemainingMs":null,"doctorStatus":"ok","doctorBadge":"normal","terminalMainLoopContinues":true,"remoteTriggerAllowed":false}"#,
    ]
    .join("\n");
    let path = McpPoolStoreRuntime::resolve_store_path(root);
    fs::create_dir_all(path.parent().expect("MCP pool 路径应有父目录"))
        .expect("MCP pool 目录应可创建");
    fs::write(
        ShardedJsonlStore::physical_shard_path(path, 0),
        format!("{content}\n"),
    )
    .expect("测试 MCP 会话池记录应可写入");
}

fn seed_handshake_mcp_config(root: &Path, server_id: &str) {
    let server_root = root.join("mcp-test-runtime").join(server_id);
    fs::create_dir_all(&server_root).expect("测试 MCP server 目录应可创建");
    let (command, script_path) = mcp_cli_test_fixtures::handshake_command(&server_root);
    fs::write(&script_path, mcp_cli_test_fixtures::handshake_script())
        .expect("测试 MCP server 脚本应可写入");
    make_executable(&script_path);
    write_mcp_server_config(root, server_id, stdio_config_json(&command));
}

fn seed_runtime_exec_mcp_config(root: &Path, server_id: &str) {
    let server_root = root.join("mcp-test-runtime").join(server_id);
    fs::create_dir_all(&server_root).expect("运行时 MCP server 目录应可创建");
    let (command, script_path) = mcp_cli_test_fixtures::runtime_exec_command(&server_root);
    fs::write(&script_path, mcp_cli_test_fixtures::runtime_exec_script())
        .expect("运行时 MCP server 脚本应可写入");
    make_executable(&script_path);
    write_mcp_server_config(root, server_id, stdio_config_json(&command));
}

/// 以指定 `tools/call` 超时装配运行时 MCP server，用于验证调用期失败语义。
fn seed_runtime_exec_mcp_config_with_tool_timeout(root: &Path, server_id: &str, timeout_ms: u64) {
    let server_root = root.join("mcp-test-runtime").join(server_id);
    fs::create_dir_all(&server_root).expect("运行时 MCP server 目录应可创建");
    let (command, script_path) = mcp_cli_test_fixtures::runtime_exec_command(&server_root);
    fs::write(&script_path, mcp_cli_test_fixtures::runtime_exec_script())
        .expect("运行时 MCP server 脚本应可写入");
    make_executable(&script_path);
    let mut config = stdio_config_json(&command);
    config["toolTimeoutMs"] = serde_json::json!(timeout_ms);
    write_mcp_server_config(root, server_id, config);
}

fn seed_malformed_resources_mcp_config(root: &Path, server_id: &str) {
    let server_root = root.join("mcp-test-runtime").join(server_id);
    fs::create_dir_all(&server_root).expect("异常 MCP server 目录应可创建");
    let (command, script_path) = mcp_cli_test_fixtures::malformed_resources_command(&server_root);
    fs::write(
        &script_path,
        mcp_cli_test_fixtures::malformed_resources_script(),
    )
    .expect("异常资源 MCP server 脚本应可写入");
    make_executable(&script_path);
    write_mcp_server_config(root, server_id, stdio_config_json(&command));
}

fn seed_collision_mcp_config(root: &Path, server_id: &str) {
    let server_root = root.join("mcp-test-runtime").join(server_id);
    fs::create_dir_all(&server_root).expect("碰撞 MCP server 目录应可创建");
    let (command, script_path) = mcp_cli_test_fixtures::collision_command(&server_root);
    fs::write(&script_path, mcp_cli_test_fixtures::collision_script())
        .expect("碰撞 MCP server 脚本应可写入");
    make_executable(&script_path);
    write_mcp_server_config(root, server_id, stdio_config_json(&command));
}

fn write_mcp_server_config(root: &Path, server_id: &str, config: Value) {
    fs::create_dir_all(root).expect("测试 Axon 根目录应可创建");
    let path = root.join(".mcp.json");
    let mut document = fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({"mcpServers": {}}));
    document["mcpServers"][server_id] = config;
    fs::write(
        path,
        serde_json::to_string_pretty(&document).expect("测试 MCP 配置应可序列化"),
    )
    .expect("测试 MCP 配置应可写入");
}

fn stdio_config_json(command: &[String]) -> serde_json::Value {
    serde_json::json!({
        "type": "stdio",
        "command": command[0],
        "args": &command[1..],
    })
}

fn test_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root().join(format!(
        "axon-mcp-cli-{name}-{}-{:?}-{nonce}",
        std::process::id(),
        std::thread::current().id()
    ))
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(path)
        .expect("脚本 metadata 应可读取")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("脚本权限应可设置");
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}
