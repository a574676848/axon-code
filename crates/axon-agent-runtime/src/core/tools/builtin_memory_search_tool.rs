use axon_domain::{
    ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    ToolSchemaFieldKind,
};

/// 内置 memory_search tool 名称。
pub const MEMORY_SEARCH_TOOL_NAME: &str = "memory_search";
/// memory_search tool schema version。
pub const MEMORY_SEARCH_TOOL_SCHEMA_VERSION: u32 = 1;
/// memory_search tool ID。
pub const MEMORY_SEARCH_TOOL_ID: ToolId = ToolId(17);
/// memory_search tool 必填 query 字段。
pub const MEMORY_SEARCH_TOOL_QUERY_FIELD: &str = "query";
/// memory_search tool 可选 limit 字段。
pub const MEMORY_SEARCH_TOOL_LIMIT_FIELD: &str = "limit";
/// memory_search 默认最大返回条目数。
pub const MEMORY_SEARCH_DEFAULT_LIMIT: usize = 5;

pub(crate) fn memory_search_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        MEMORY_SEARCH_TOOL_SCHEMA_VERSION,
        [MEMORY_SEARCH_TOOL_QUERY_FIELD],
        vec![
            ToolSchemaField::new(
                MEMORY_SEARCH_TOOL_QUERY_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "要从长期 Memory 中检索的非空问题、决策或偏好。",
            ),
            ToolSchemaField::new(
                MEMORY_SEARCH_TOOL_LIMIT_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选最大返回条目数，必须为正整数，默认 5。",
            ),
        ],
    );
    schema.input_schema["properties"][MEMORY_SEARCH_TOOL_LIMIT_FIELD]["minimum"] =
        serde_json::json!(1);
    ToolDefinition {
        id: MEMORY_SEARCH_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: MEMORY_SEARCH_TOOL_NAME.to_owned(),
        description:
            "当过去的用户偏好、项目决策或经验可能影响当前任务时，描述具体问题并检索相关长期记忆。"
                .to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}
