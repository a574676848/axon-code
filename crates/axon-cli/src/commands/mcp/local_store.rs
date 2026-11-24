use std::{
    fs,
    path::{Path, PathBuf},
};

use axon_agent_runtime::{McpServerConfig, mcp_server_id};
use axon_domain::MCP_CONFIG_FILE;
use serde_json::{Map, Value};

use crate::mcp_config_file::mcp_config_value;

const MCP_SERVERS_KEY: &str = "mcpServers";

pub(crate) fn write_user_mcp_server(
    root: &Path,
    server_id: &str,
    config: &McpServerConfig,
) -> Result<PathBuf, String> {
    let path = root.join(MCP_CONFIG_FILE);
    let mut document = read_document(&path)?;
    let servers = mcp_servers_mut(&mut document)?;
    servers.insert(server_id.to_owned(), mcp_config_value(config)?);
    write_document(root, &path, &document)?;
    Ok(path)
}

pub(crate) fn remove_user_mcp_server(root: &Path, server_id: &str) -> Result<PathBuf, String> {
    let path = root.join(MCP_CONFIG_FILE);
    let mut document = read_document(&path)?;
    let servers = mcp_servers_mut(&mut document)?;
    let configured_name = servers
        .keys()
        .find(|name| mcp_server_id(name).ok().as_deref() == Some(server_id))
        .cloned();
    if configured_name
        .as_deref()
        .and_then(|name| servers.remove(name))
        .is_none()
    {
        return Err(format!("用户级 MCP server 不存在：{server_id}"));
    }
    write_document(root, &path, &document)?;
    Ok(path)
}

fn read_document(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let raw = fs::read_to_string(path).map_err(|_| "用户级 MCP 配置读取失败".to_owned())?;
    let document =
        serde_json::from_str::<Value>(&raw).map_err(|_| "用户级 MCP 配置 JSON 无效".to_owned())?;
    if !document.is_object() {
        return Err("用户级 MCP 配置根节点必须是对象".to_owned());
    }
    Ok(document)
}

fn mcp_servers_mut(document: &mut Value) -> Result<&mut Map<String, Value>, String> {
    let root = document
        .as_object_mut()
        .ok_or_else(|| "用户级 MCP 配置根节点必须是对象".to_owned())?;
    let servers = root
        .entry(MCP_SERVERS_KEY)
        .or_insert_with(|| Value::Object(Map::new()));
    servers
        .as_object_mut()
        .ok_or_else(|| "用户级 MCP 配置 mcpServers 必须是对象".to_owned())
}

fn write_document(root: &Path, path: &Path, document: &Value) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|_| "用户级 Axon 目录创建失败".to_owned())?;
    let raw = serde_json::to_string_pretty(document)
        .map_err(|_| "用户级 MCP 配置序列化失败".to_owned())?;
    fs::write(path, format!("{raw}\n")).map_err(|_| "用户级 MCP 配置写入失败".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_agent_runtime::{McpServerCommand, McpServerConfig};

    #[test]
    fn add_and_remove_preserve_unrelated_configuration() {
        let root = axon_test_support::test_temp_root()
            .join(format!("axon-mcp-user-store-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join(MCP_CONFIG_FILE),
            r#"{"other":true,"mcpServers":{"existing":{"command":"existing"}}}"#,
        )
        .unwrap();
        let config = McpServerConfig::Stdio(McpServerCommand {
            program: "new-server".to_owned(),
            args: Vec::new(),
            working_dir: None,
            env: Default::default(),
            tool_timeout_ms: None,
        });

        write_user_mcp_server(&root, "new", &config).unwrap();
        remove_user_mcp_server(&root, "new").unwrap();

        let value: Value =
            serde_json::from_str(&fs::read_to_string(root.join(MCP_CONFIG_FILE)).unwrap()).unwrap();
        assert_eq!(value["other"], true);
        assert!(value["mcpServers"]["existing"].is_object());
        assert!(value["mcpServers"].get("new").is_none());
        let _ = fs::remove_dir_all(root);
    }
}
