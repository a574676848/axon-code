//! 工具展示字段的 surface 层辅助函数。

const MCP_TOOL_PREFIX: &str = "mcp__";
const MCP_TOOL_DELIMITER: &str = "__";

pub(crate) fn mcp_tool_parts(value: &str) -> (String, Option<String>) {
    let Some(body) = value.trim().strip_prefix(MCP_TOOL_PREFIX) else {
        return (String::new(), None);
    };
    let Some((server, tool)) = body.split_once(MCP_TOOL_DELIMITER) else {
        return (String::new(), None);
    };
    (server.to_owned(), non_empty_string(tool.to_owned()))
}

pub(crate) fn mcp_catalog_target(server: &str, tool: Option<&str>) -> Option<String> {
    tool.map(|tool| format!("{server}.{tool}"))
        .or_else(|| non_empty_string(server.to_owned()))
}

pub(crate) fn mcp_catalog_raw_name(server: &str, tool: Option<&str>) -> String {
    tool.map_or_else(|| server.to_owned(), |tool| format!("{server}.{tool}"))
}

pub(crate) fn non_empty_string(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}
