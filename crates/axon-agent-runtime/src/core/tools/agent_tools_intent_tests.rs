//! 折叠态规则表与真实工具 schema 的一致性用例。
//!
//! 单独成文件：它横跨全部内置工具，与 `agent_tools_tests` 的单工具细节用例不属于同一职责。

use super::*;

/// 名单里的工具都必须声明必填 `intent`，声明了 `intent` 的工具也必须在名单里。
#[test]
fn intent_tool_list_matches_declared_schemas() {
    let mut registry = ToolRegistry::new();
    AgentToolRegistryRuntime::register_builtin_tools(&mut registry).unwrap();
    let declared = crate::tool_intent_tool_names();

    for tool in registry.tools() {
        let requires_intent = tool
            .schema
            .required_field_names()
            .iter()
            .any(|field| field == crate::TOOL_INTENT_FIELD);
        assert_eq!(
            requires_intent,
            declared.iter().any(|name| name == &tool.name),
            "{} 的 intent 契约必须与折叠态规则表一致",
            tool.name
        );
    }
    for name in declared {
        assert!(
            registry.tools().iter().any(|tool| &tool.name == name),
            "折叠态规则表里的 {name} 必须是已注册的内置工具"
        );
    }
}
