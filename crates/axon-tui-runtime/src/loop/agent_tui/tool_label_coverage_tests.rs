//! 用真实注册清单验证共享名称表与 TUI 投影，新增工具时防止漏译。

use axon_agent_runtime::{AgentToolRegistryRuntime, ToolRegistry};

use super::*;

#[test]
fn every_registered_builtin_has_a_chinese_label() {
    let report = AgentToolRegistryRuntime::register_builtin_tools(&mut ToolRegistry::new())
        .expect("注册内置工具失败");
    let tools = report.registered_tools.into_iter();
    for tool in tools {
        let name = format!("{}.{}", tool.namespace, tool.name);
        let label = builtin_tool_display_label(&name)
            .unwrap_or_else(|| panic!("内置工具缺少中文标签：{name}"));
        assert!(!label.is_ascii(), "{name}");
        let kind = AgentTuiToolKind::from_tool_name(&name);
        let running = ToolDisplayProjection::from_tool_call(&name, kind, "{}", None);
        let completed = ToolDisplayProjection::from_tool(&name, kind, None);
        if tool.name == "write_stdin" {
            assert_eq!(running.label, LABEL_WAITING_BACKGROUND_TERMINAL);
            assert_eq!(completed.label, LABEL_BACKGROUND_TERMINAL);
        } else {
            assert_eq!(running.label, label, "执行中：{name}");
            assert_eq!(completed.label, label, "已完成：{name}");
        }
    }
}

#[test]
fn mcp_and_unknown_tools_do_not_inherit_builtin_labels() {
    for (name, kind, expected) in [
        (
            "builtin.task_future",
            AgentTuiToolKind::Unsupported,
            "builtin.task_future",
        ),
        (
            "builtin.notes_future",
            AgentTuiToolKind::Notes,
            "builtin.notes_future",
        ),
        (
            "plugin.update_plan",
            AgentTuiToolKind::Unsupported,
            "plugin.update_plan",
        ),
        (
            "mcp__server__write_stdin",
            AgentTuiToolKind::Unsupported,
            LABEL_MCP,
        ),
        ("write_stdin", AgentTuiToolKind::Mcp, LABEL_MCP),
        ("update_plan", AgentTuiToolKind::Mcp, LABEL_MCP),
    ] {
        assert_eq!(
            ToolDisplayProjection::from_tool_call(name, kind, "{}", None).label,
            expected
        );
        assert_eq!(
            ToolDisplayProjection::from_tool(name, kind, None).label,
            expected
        );
    }
}
