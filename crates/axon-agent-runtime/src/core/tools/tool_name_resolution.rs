//! Provider tool name 的精确 wire 投影还原。

use crate::{
    EDIT_TOOL_CUSTOM_WIRE_NAME, EDIT_TOOL_QUALIFIED_NAME, ToolRegistry, provider_tool_name,
};

/// 工具名解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolNameResolution {
    /// 本机规范工具名；未命中当前工具列表时保留 provider 原值。
    pub canonical_name: String,
}

/// 依据当前 registry 精确还原 provider 工具名。
#[must_use]
pub fn resolve_provider_tool_name(registry: &ToolRegistry, name: &str) -> ToolNameResolution {
    let candidates = registry
        .model_tools()
        .into_iter()
        .map(|tool| tool.qualified_name)
        .collect::<Vec<_>>();
    resolve_provider_tool_name_candidates(&candidates, name)
}

/// 依据模型可见工具名列表精确还原 provider wire 名称。
#[must_use]
pub fn resolve_provider_tool_name_candidates(
    candidates: &[String],
    name: &str,
) -> ToolNameResolution {
    let canonical_name = candidates
        .iter()
        .find(|candidate| {
            candidate.as_str() == name
                || provider_tool_name(candidate.as_str()) == name
                || provider_custom_tool_name(candidate.as_str()) == name
        })
        .cloned()
        .unwrap_or_else(|| name.to_owned());
    ToolNameResolution { canonical_name }
}

/// 将本机工具名投影为 Responses custom tool 的 wire 名称。
#[must_use]
pub fn provider_custom_tool_name(qualified_name: &str) -> String {
    if qualified_name == EDIT_TOOL_QUALIFIED_NAME {
        return EDIT_TOOL_CUSTOM_WIRE_NAME.to_owned();
    }
    provider_tool_name(qualified_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentToolRegistryRuntime;

    #[test]
    fn restores_only_the_exact_provider_projection() {
        let mut registry = ToolRegistry::new();
        AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();

        assert_eq!(
            resolve_provider_tool_name(&registry, "builtin_shell_command").canonical_name,
            "builtin.shell_command"
        );
        assert_eq!(
            resolve_provider_tool_name(&registry, "builtin.shell_command").canonical_name,
            "builtin.shell_command"
        );
        assert_eq!(
            resolve_provider_tool_name(&registry, "builtin_shell_commnd").canonical_name,
            "builtin_shell_commnd"
        );
        assert_eq!(
            resolve_provider_tool_name(&registry, "shell").canonical_name,
            "shell"
        );
        assert_eq!(
            resolve_provider_tool_name_candidates(
                &["mcp__team_tools__query_db".to_owned()],
                "mcp__team_tools__query_db"
            )
            .canonical_name,
            "mcp__team_tools__query_db"
        );
    }

    #[test]
    fn restores_edit_custom_wire_name() {
        assert_eq!(
            provider_custom_tool_name(EDIT_TOOL_QUALIFIED_NAME),
            EDIT_TOOL_CUSTOM_WIRE_NAME
        );
        assert_eq!(
            resolve_provider_tool_name_candidates(
                &[EDIT_TOOL_QUALIFIED_NAME.to_owned()],
                EDIT_TOOL_CUSTOM_WIRE_NAME,
            )
            .canonical_name,
            EDIT_TOOL_QUALIFIED_NAME
        );
    }
}
