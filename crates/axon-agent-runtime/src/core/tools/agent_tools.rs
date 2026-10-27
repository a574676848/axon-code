//! 内置 Agent tool 注册与本机 action 提案。

use axon_domain::{
    AxonResult, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema,
    ToolSchemaField, ToolSchemaFieldKind,
};
use serde_json::json;

use crate::ToolRegistry;
use crate::builtin_ask_user_question_tool::ask_user_question_tool_definition;
use crate::builtin_edit_tool::edit_tool_definition;
use crate::builtin_fs_tools::read_file_tool_definition;
use crate::builtin_glob_tool::glob_tool_definition;
use crate::builtin_goal_tools::goal_tool_definitions;
use crate::builtin_history_tools::history_tool_definitions;
use crate::builtin_job_tools::{job_kill_tool_definition, job_output_tool_definition};
use crate::builtin_memory_search_tool::memory_search_tool_definition;
use crate::builtin_memory_write_tool::memory_write_tool_definition;
use crate::builtin_notes_tools::notes_tool_definitions;
use crate::builtin_search_tool::search_tool_definition;
use crate::builtin_shell_tool::{shell_command_tool_definition, write_stdin_tool_definition};
use crate::builtin_skill_invoke_tool::invoke_skill_tool_definition;
use crate::builtin_subagent_tools::subagent_tool_definitions;
use crate::builtin_token_budget_tools::token_budget_tool_definitions;
use crate::builtin_update_plan_tool::update_plan_tool_definition;

/// 内置 Agent tool namespace。
pub const AGENT_TOOL_NAMESPACE: &str = "builtin";
const LIST_MCP_RESOURCES_TOOL_ID: ToolId = ToolId(32);
const READ_MCP_RESOURCE_TOOL_ID: ToolId = ToolId(34);

/// 内置 Agent tool 注册报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentToolRegistrationReport {
    /// 注册到 `ToolRegistry` 的工具定义。
    pub registered_tools: Vec<ToolDefinition>,
}

/// 内置 Agent tool runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct AgentToolRegistryRuntime;

impl AgentToolRegistryRuntime {
    /// 注册内置 Agent tools。
    pub fn register_builtin_tools(
        registry: &mut ToolRegistry,
    ) -> AxonResult<AgentToolRegistrationReport> {
        let mut tools = vec![
            search_tool_definition(),
            memory_search_tool_definition(),
            memory_write_tool_definition(),
            shell_command_tool_definition(),
            write_stdin_tool_definition(),
            job_output_tool_definition(),
            job_kill_tool_definition(),
            edit_tool_definition(),
            read_file_tool_definition(),
            glob_tool_definition(),
            invoke_skill_tool_definition(),
            update_plan_tool_definition(),
            ask_user_question_tool_definition(),
            list_mcp_resources_tool_definition(),
            read_mcp_resource_tool_definition(),
        ];
        tools.extend(goal_tool_definitions());
        tools.extend(token_budget_tool_definitions());
        tools.extend(notes_tool_definitions());
        tools.extend(history_tool_definitions());
        let tools = tools
            .into_iter()
            .chain(subagent_tool_definitions())
            .map(with_parallel_note)
            .collect::<Vec<_>>();
        let mut candidate = registry.clone();
        for tool in &tools {
            candidate.register(tool.clone())?;
        }
        *registry = candidate;

        Ok(AgentToolRegistrationReport {
            registered_tools: tools,
        })
    }

    /// 批量调度 provider tool-call；只生成稳定编号与执行边界，不直接执行副作用。
    pub fn schedule_provider_tool_calls(
        registry: &ToolRegistry,
        input: crate::ProviderToolCallScheduleInput,
    ) -> AxonResult<crate::ProviderToolCallScheduleReport> {
        crate::ProviderToolCallScheduleRuntime::schedule(registry, input)
    }
}

/// 只读、可与其他只读调用同批发出的工具（PAR-1 第 2 节）。
const PARALLEL_CALL_TOOLS: [&str; 8] = [
    "read_file",
    "glob",
    "search",
    "memory_search",
    "list_agents",
    "job_output",
    "list_mcp_resources",
    "read_mcp_resource",
];

/// 会改变运行态或工作区、必须单独一批的工具。
const EXCLUSIVE_CALL_TOOLS: [&str; 7] = [
    "edit",
    "write_stdin",
    "job_kill",
    "update_plan",
    "ask_user_question",
    "send_message",
    "memory_write",
];

/// 给工具描述补一句并行属性，模型据此决定同批还是单发。
fn with_parallel_note(mut tool: ToolDefinition) -> ToolDefinition {
    let note = if PARALLEL_CALL_TOOLS.contains(&tool.name.as_str()) {
        "只读，可与其他只读调用同批发出。"
    } else if EXCLUSIVE_CALL_TOOLS.contains(&tool.name.as_str()) {
        "会改变运行态或工作区，必须单独一批。"
    } else if tool.name == "shell_command" {
        "只读命令可与其他只读调用同批；写命令必须单独一批。"
    } else {
        return tool;
    };
    if !tool.description.contains(note) {
        tool.description.push('（');
        tool.description.push_str(note);
        tool.description.push('）');
    }
    tool
}

fn list_mcp_resources_tool_definition() -> ToolDefinition {
    ToolDefinition {
        id: LIST_MCP_RESOURCES_TOOL_ID,
        namespace: AGENT_TOOL_NAMESPACE.to_owned(),
        name: "list_mcp_resources".to_owned(),
        description: "需要读取 MCP 提供的外部上下文但还不知道 URI 时，列出可用资源；随后用 read_mcp_resource 读取。".to_owned(),
        schema: mcp_list_tool_schema(),
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn read_mcp_resource_tool_definition() -> ToolDefinition {
    let schema = ToolSchema::object(
        1,
        ["server", "uri"],
        vec![
            schema_field(
                "server",
                ToolSchemaFieldKind::NonBlankString,
                "MCP server ID。",
            ),
            schema_field(
                "uri",
                ToolSchemaFieldKind::NonBlankString,
                "资源列表返回的完整 URI。",
            ),
        ],
    );
    ToolDefinition {
        id: READ_MCP_RESOURCE_TOOL_ID,
        namespace: AGENT_TOOL_NAMESPACE.to_owned(),
        name: "read_mcp_resource".to_owned(),
        description: "已有资源列表返回的完整 URI 时，读取该 MCP 资源的实际内容。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn mcp_list_tool_schema() -> ToolSchema {
    let mut schema = ToolSchema::object(
        1,
        std::iter::empty::<&str>(),
        vec![
            schema_field(
                "server",
                ToolSchemaFieldKind::NonBlankString,
                "可选 MCP server ID；省略时列出全部 server。提供 cursor 时必须同时提供同一 server。",
            ),
            schema_field(
                "cursor",
                ToolSchemaFieldKind::NonBlankString,
                "可选分页 cursor；只能复用上一页同一 server 返回的 cursor。提供 cursor 时必须同时提供 server。",
            ),
        ],
    );
    schema.input_schema["allOf"] = json!([{
        "if": { "required": ["cursor"] },
        "then": { "required": ["server"] }
    }]);
    schema.input_schema["examples"] = json!([
        {},
        { "server": "github", "cursor": "next-page-token" }
    ]);
    schema
}

fn schema_field(name: &str, kind: ToolSchemaFieldKind, description: &str) -> ToolSchemaField {
    ToolSchemaField::new(name, kind, description)
}

#[cfg(test)]
#[path = "agent_tools_tests.rs"]
mod tests;
