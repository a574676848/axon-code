//! Token Budget 感知层工具定义。

use axon_domain::{
    ToolConcurrencyPolicy, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};

/// 查询当前窗口剩余预算的工具名称。
pub const GET_CONTEXT_REMAINING_TOOL_NAME: &str = "get_context_remaining";
/// 查询当前窗口剩余预算的完整名称。
pub const GET_CONTEXT_REMAINING_TOOL_QUALIFIED_NAME: &str = "builtin.get_context_remaining";
/// 请求开启新记忆窗口的工具名称。
pub const NEW_CONTEXT_TOOL_NAME: &str = "new_context";
/// 请求开启新记忆窗口的完整名称。
pub const NEW_CONTEXT_TOOL_QUALIFIED_NAME: &str = "builtin.new_context";

const GET_CONTEXT_REMAINING_TOOL_ID: ToolId = ToolId(50);
const NEW_CONTEXT_TOOL_ID: ToolId = ToolId(51);

/// 注册到 provider 的 Token Budget 工具。
#[must_use]
pub fn token_budget_tool_definitions() -> [ToolDefinition; 2] {
    [
        get_context_remaining_tool_definition(),
        new_context_tool_definition(),
    ]
}

fn get_context_remaining_tool_definition() -> ToolDefinition {
    ToolDefinition {
        id: GET_CONTEXT_REMAINING_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: GET_CONTEXT_REMAINING_TOOL_NAME.to_owned(),
        description:
            "获取当前记忆窗口距离自动换窗还剩多少 token，并说明数值来自 Provider 实测还是本地估算。"
                .to_owned(),
        schema: ToolSchema::object(1, std::iter::empty::<&str>(), Vec::new()),
        enabled: true,
        concurrency_policy: ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

fn new_context_tool_definition() -> ToolDefinition {
    ToolDefinition {
        id: NEW_CONTEXT_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: NEW_CONTEXT_TOOL_NAME.to_owned(),
        description: "当前进度已写入私有便签且适合换窗时，请求在本次工具调用结束后开启新的记忆窗口；尚未成功写入便签时本次调用会被拒绝，并返回需要补写内容的可执行指令，可在同一轮补写后重试。旧消息仍可按需回查。".to_owned(),
        schema: ToolSchema::object(
            1,
            std::iter::empty::<&str>(),
            vec![ToolSchemaField::new(
                "reason",
                ToolSchemaFieldKind::String,
                "可选。简要说明当前为何适合开启新窗口。",
            )],
        ),
        enabled: true,
        concurrency_policy: ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}
