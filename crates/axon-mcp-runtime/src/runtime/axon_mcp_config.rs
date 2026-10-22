use std::{collections::BTreeMap, fs, path::Path};

use axon_agent_runtime::{McpHttpServerConfig, McpServerCommand, McpServerConfig, McpServerHeader};
use serde_json::Value;

/// 第三方 MCP 客户端（VS Code、Cursor 等）常用但 Axon 不直接消费的键。
/// 保留在白名单中以避免拒绝合法配置。
const MCP_PASSTHROUGH_KEYS: &[&str] = &["disabled", "timeout", "timeoutMs", "timeout_ms"];

const MCP_COMMON_CONFIG_KEYS: &[&str] = &["type", "toolTimeoutMs", "tool_timeout_ms"];
const MCP_STDIO_CONFIG_KEYS: &[&str] =
    &["command", "args", "cwd", "workingDir", "working_dir", "env"];
const MCP_STREAMABLE_HTTP_CONFIG_KEYS: &[&str] = &[
    "url",
    "headers",
    "bearerTokenEnvVar",
    "bearer_token_env_var",
    "bearer_token_env",
];
const MCP_TYPE_STDIO: &str = "stdio";
const MCP_TYPE_STREAMABLE_HTTP: &str = "streamable_http";
const MCP_TYPE_HTTP: &str = "http";

pub(crate) fn mcp_servers_from_path(path: &Path) -> Vec<(String, McpServerConfig)> {
    let Ok(raw) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let mut servers = Vec::new();
    push_mcp_servers(&value, &mut servers);
    servers
}

fn push_mcp_servers(value: &Value, servers: &mut Vec<(String, McpServerConfig)>) {
    let Some(items) = value.get("mcpServers").and_then(Value::as_object) else {
        return;
    };
    for (name, config) in items {
        if let Some(config) = mcp_config_from_value(config, name) {
            servers.push((name.clone(), config));
        } else {
            axon_logger::warning(&format!(
                "mcp_config_load_failed server={name} (无法解析 MCP server 配置，已跳过)"
            ));
        }
    }
}

fn mcp_config_from_value(value: &Value, server_name: &str) -> Option<McpServerConfig> {
    let mcp_type = normalized_mcp_type(value);
    match mcp_type {
        MCP_TYPE_STDIO if mcp_config_keys_valid(value, MCP_STDIO_CONFIG_KEYS, server_name) => {
            mcp_stdio_config_from_value(value)
        }
        MCP_TYPE_STREAMABLE_HTTP
            if mcp_config_keys_valid(value, MCP_STREAMABLE_HTTP_CONFIG_KEYS, server_name) =>
        {
            mcp_streamable_http_config_from_value(value)
        }
        _ => {
            axon_logger::warning(&format!(
                "mcp_config_skip server={server_name} type={mcp_type} (不支持的 MCP server 类型)"
            ));
            None
        }
    }
}

fn normalized_mcp_type(value: &Value) -> &str {
    match value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(MCP_TYPE_STDIO)
    {
        MCP_TYPE_HTTP => MCP_TYPE_STREAMABLE_HTTP,
        other => other,
    }
}

fn mcp_config_keys_valid(value: &Value, allowed: &[&str], server_name: &str) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    for key in object.keys() {
        let k = key.as_str();
        if MCP_COMMON_CONFIG_KEYS.contains(&k)
            || allowed.contains(&k)
            || MCP_PASSTHROUGH_KEYS.contains(&k)
        {
            continue;
        }
        axon_logger::warning(&format!(
            "mcp_config_unknown_key server={server_name} key={k} (已忽略)"
        ));
    }
    true
}

fn mcp_stdio_config_from_value(value: &Value) -> Option<McpServerConfig> {
    let program = value.get("command")?.as_str()?.trim();
    if program.is_empty() {
        return None;
    }
    let args = value
        .get("args")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let working_dir = value
        .get("cwd")
        .or_else(|| value.get("workingDir"))
        .or_else(|| value.get("working_dir"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    Some(McpServerConfig::Stdio(McpServerCommand {
        program: program.to_owned(),
        args,
        working_dir,
        env: stdio_env_from_value(value),
        tool_timeout_ms: tool_timeout_ms(value),
    }))
}

fn stdio_env_from_value(value: &Value) -> BTreeMap<String, String> {
    value
        .get("env")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(name, value)| {
                    let value = value.as_str()?;
                    (!name.trim().is_empty()).then(|| (name.clone(), value.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn mcp_streamable_http_config_from_value(value: &Value) -> Option<McpServerConfig> {
    let url = value.get("url")?.as_str()?.trim();
    if url.is_empty() {
        return None;
    }
    let headers = value
        .get("headers")
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(name, value)| {
                    let value = value.as_str()?;
                    if name.trim().is_empty() || value.trim().is_empty() {
                        return None;
                    }
                    Some(McpServerHeader {
                        name: name.clone(),
                        value: value.to_owned(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(McpServerConfig::StreamableHttp(McpHttpServerConfig {
        url: url.to_owned(),
        headers,
        bearer_token_env_var: value
            .get("bearerTokenEnvVar")
            .or_else(|| value.get("bearer_token_env_var"))
            .or_else(|| value.get("bearer_token_env"))
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned),
        oauth_access_token: None,
        tool_timeout_ms: tool_timeout_ms(value),
    }))
}

fn tool_timeout_ms(value: &Value) -> Option<u64> {
    value
        .get("toolTimeoutMs")
        .or_else(|| value.get("tool_timeout_ms"))
        .or_else(|| value.get("timeout"))
        .or_else(|| value.get("timeoutMs"))
        .or_else(|| value.get("timeout_ms"))
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_missing_type_to_stdio_and_reads_env() {
        let root = test_root("default-stdio-env");
        let config = root.join(".mcp.json");
        fs::create_dir_all(&root).expect("测试目录应可创建");
        fs::write(
            &config,
            r#"{"mcpServers":{"gitnexus":{"command":"gitnexus","args":["serve"],"env":{"TOKEN":"hidden"}}}}"#,
        )
        .expect("测试 MCP 配置应可写入");

        let servers = mcp_servers_from_path(&config);

        assert_eq!(servers.len(), 1);
        match &servers[0].1 {
            McpServerConfig::Stdio(command) => {
                assert_eq!(command.program, "gitnexus");
                assert_eq!(command.args, vec!["serve"]);
                assert_eq!(command.env.get("TOKEN").map(String::as_str), Some("hidden"));
            }
            McpServerConfig::StreamableHttp(_) => panic!("缺省 type 应解析为 stdio"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn treats_http_type_as_streamable_http() {
        let root = test_root("http-alias");
        let config = root.join(".mcp.json");
        fs::create_dir_all(&root).expect("测试目录应可创建");
        fs::write(
            &config,
            r#"{"mcpServers":{"openviking":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"X-Test":"ok"}}}}"#,
        )
        .expect("测试 MCP 配置应可写入");

        let servers = mcp_servers_from_path(&config);

        assert_eq!(servers.len(), 1);
        match &servers[0].1 {
            McpServerConfig::StreamableHttp(http) => {
                assert_eq!(http.url, "http://127.0.0.1:9000/mcp");
                assert_eq!(http.headers[0].name, "X-Test");
            }
            McpServerConfig::Stdio(_) => panic!("type=http 应解析为 streamable_http"),
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignores_removed_trusted_key_and_still_loads_server() {
        let root = test_root("trusted-removed");
        let config = root.join(".mcp.json");
        fs::create_dir_all(&root).expect("测试目录应可创建");
        fs::write(
            &config,
            r#"{"mcpServers":{"legacy":{"command":"legacy-mcp","trusted":true}}}"#,
        )
        .expect("测试 MCP 配置应可写入");

        let servers = mcp_servers_from_path(&config);

        // trusted 已从配置契约移除：旧配置仍能加载，该键不再影响 MCP 权限映射。
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "legacy");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn ignores_unknown_keys_and_still_loads_server() {
        let root = test_root("unknown-keys");
        let config = root.join(".mcp.json");
        fs::create_dir_all(&root).expect("测试目录应可创建");
        fs::write(
            &config,
            r#"{"mcpServers":{"db":{"type":"stdio","command":"db-mcp","legacy_unknown_policy":"allow","timeout":60000,"disabled":false}}}"#,
        )
        .expect("测试 MCP 配置应可写入");

        let servers = mcp_servers_from_path(&config);

        // 未知键（legacy_unknown_policy）应被跳过，不影响 server 加载；
        // timeout/disabled 等常见键被兼容。
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].0, "db");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reads_common_timeout_key_as_tool_timeout() {
        let root = test_root("common-timeout");
        let config = root.join(".mcp.json");
        fs::create_dir_all(&root).expect("测试目录应可创建");
        fs::write(
            &config,
            r#"{"mcpServers":{"http-svc":{"type":"http","url":"https://example.com/mcp","timeout":60000,"disabled":false}}}"#,
        )
        .expect("测试 MCP 配置应可写入");

        let servers = mcp_servers_from_path(&config);

        assert_eq!(servers.len(), 1);
        match &servers[0].1 {
            McpServerConfig::StreamableHttp(http) => {
                assert_eq!(http.url, "https://example.com/mcp");
                assert_eq!(http.tool_timeout_ms, Some(60000));
            }
            McpServerConfig::Stdio(_) => panic!("type=http 应解析为 streamable_http"),
        }
        let _ = fs::remove_dir_all(root);
    }

    fn test_root(name: &str) -> std::path::PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应晚于 UNIX_EPOCH")
            .as_nanos();
        axon_test_support::test_temp_root().join(format!(
            "axon-mcp-config-{name}-{}-{nonce}",
            std::process::id()
        ))
    }
}
