//! MCP 工具唯一模型/provider 命名规则。

use axon_domain::{AxonError, AxonResult};

/// MCP 工具名称前缀。
pub const MCP_TOOL_NAMESPACE_PREFIX: &str = "mcp__";
/// MCP server 与 tool 的唯一分隔符。
pub const MCP_TOOL_PROVIDER_DELIMITER: &str = "__";
/// provider 可见 MCP 工具完整名称最大长度。
pub const MCP_PROVIDER_TOOL_NAME_MAX_LEN: usize = 64;
const MCP_TOOL_HASH_LEN: usize = 12;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// 将 MCP server 原始名称规范为稳定的 server ID。
pub fn mcp_server_id(value: &str) -> AxonResult<String> {
    normalize_mcp_component(value, "server")
}

/// 将 MCP tool 原始名称规范为稳定的 tool ID。
pub fn mcp_tool_id(value: &str) -> AxonResult<String> {
    normalize_mcp_component(value, "tool")
}

/// 由 MCP server ID 生成内部 registry namespace。
#[must_use]
pub fn mcp_tool_namespace(server_id: &str) -> String {
    format!("{MCP_TOOL_NAMESPACE_PREFIX}{server_id}")
}

/// 判断 namespace 是否属于 MCP 工具。
#[must_use]
pub fn is_mcp_tool_namespace(namespace: &str) -> bool {
    namespace.starts_with(MCP_TOOL_NAMESPACE_PREFIX)
}

/// 判断完整工具名是否属于 MCP 工具。
#[must_use]
pub fn is_mcp_tool_name(qualified_name: &str) -> bool {
    qualified_name.starts_with(MCP_TOOL_NAMESPACE_PREFIX)
}

/// 生成唯一的 MCP 模型/provider 可见名称。
#[must_use]
pub fn mcp_provider_tool_name(server_id: &str, tool_id: &str) -> String {
    let name =
        format!("{MCP_TOOL_NAMESPACE_PREFIX}{server_id}{MCP_TOOL_PROVIDER_DELIMITER}{tool_id}");
    if name.len() <= MCP_PROVIDER_TOOL_NAME_MAX_LEN {
        return name;
    }
    let suffix = stable_hash_suffix(&format!("{server_id}\0{tool_id}"));
    let prefix_len = MCP_PROVIDER_TOOL_NAME_MAX_LEN.saturating_sub(suffix.len());
    format!(
        "{}{suffix}",
        name.chars().take(prefix_len).collect::<String>()
    )
}

/// 将本机工具名投影为 provider function name。
#[must_use]
pub fn provider_tool_name(qualified_name: &str) -> String {
    if is_mcp_tool_name(qualified_name) {
        qualified_name.to_owned()
    } else {
        qualified_name.replace('.', "_")
    }
}

fn normalize_mcp_component(value: &str, fallback_prefix: &str) -> AxonResult<String> {
    let mut normalized = String::new();
    for character in value.trim().chars() {
        if character.is_ascii_alphanumeric() {
            normalized.push(character.to_ascii_lowercase());
        } else {
            normalized.push('_');
        }
    }
    while normalized.contains("__") {
        normalized = normalized.replace("__", "_");
    }
    normalized = normalized.trim_matches('_').to_owned();
    if normalized.is_empty() {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP server/tool 名称不能为空",
        ));
    }
    if normalized.starts_with(|character: char| character.is_ascii_digit()) {
        normalized = format!("{fallback_prefix}_{normalized}");
    }
    Ok(normalized)
}

fn stable_hash_suffix(value: &str) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("_{hash:016x}")
        .chars()
        .take(MCP_TOOL_HASH_LEN + 1)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_provider_name_is_the_only_model_visible_form() {
        let server_id = mcp_server_id("Team Tools").unwrap();
        let tool_id = mcp_tool_id("Query-DB").unwrap();

        assert_eq!(server_id, "team_tools");
        assert_eq!(tool_id, "query_db");
        assert_eq!(
            mcp_provider_tool_name(&server_id, &tool_id),
            "mcp__team_tools__query_db"
        );
        assert_eq!(
            provider_tool_name("mcp__team_tools__query_db"),
            "mcp__team_tools__query_db"
        );
    }

    #[test]
    fn mcp_provider_name_limits_the_complete_name_stably() {
        let name = mcp_provider_tool_name(
            "server_with_a_name_that_is_far_too_long_for_any_provider",
            "tool_with_a_name_that_is_also_far_too_long_for_any_provider",
        );

        assert_eq!(name.len(), MCP_PROVIDER_TOOL_NAME_MAX_LEN);
        assert_eq!(
            name,
            mcp_provider_tool_name(
                "server_with_a_name_that_is_far_too_long_for_any_provider",
                "tool_with_a_name_that_is_also_far_too_long_for_any_provider",
            )
        );
    }
}
