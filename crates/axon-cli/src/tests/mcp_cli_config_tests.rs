use std::{fs, path::PathBuf};

use serde_json::Value;

use super::{AxonCli, run_parsed_cli};

#[test]
fn mcp_list_reports_user_and_workspace_configs_without_leaking_values() {
    let root = test_root("axon-configs");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("Workspace 目录应可创建");
    fs::write(
        root.join(".mcp.json"),
        r#"{"mcpServers":{"user-db":{"type":"stdio","command":"secret-user-command","args":["hidden"]}}}"#,
    )
    .expect("用户级 MCP 配置应可写入");
    fs::write(
        workspace.join(".mcp.json"),
        r#"{"mcpServers":{"workspace-db":{"type":"streamable_http","url":"http://127.0.0.1:9/mcp","headers":{"Authorization":"secret-workspace-token"}}}}"#,
    )
    .expect("Workspace MCP 配置应可写入");
    let config_path = root.join("config.toml");
    let path_arg = config_path.to_string_lossy().to_string();
    let cwd_arg = workspace.to_string_lossy().to_string();

    let cli = AxonCli::parse_from_args([
        "axon", "--config", &path_arg, "--cwd", &cwd_arg, "--format", "json", "mcp", "list",
    ]);
    let result = run_parsed_cli(&cli);
    let body: Value = serde_json::from_str(&result.stdout).expect("mcp list 应输出 JSON");

    assert_eq!(result.exit_code, 0);
    assert_eq!(body["totalServers"], 2);
    assert_eq!(body["servers"][0]["serverId"], "user_db");
    assert_eq!(body["servers"][0]["source"], "user");
    assert!(
        body["servers"][0]["configRef"]
            .as_str()
            .expect("configRef 应为字符串")
            .contains(".mcp.json")
    );
    assert_eq!(body["servers"][1]["serverId"], "workspace_db");
    assert_eq!(body["servers"][1]["source"], "workspace");
    assert!(!result.stdout.contains("secret-user-command"));
    assert!(!result.stdout.contains("secret-workspace-token"));
    let _ = fs::remove_dir_all(root);
}

fn test_root(name: &str) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间应晚于 UNIX_EPOCH")
        .as_nanos();
    axon_test_support::test_temp_root().join(format!(
        "axon-mcp-cli-config-{name}-{}-{nonce}",
        std::process::id()
    ))
}
