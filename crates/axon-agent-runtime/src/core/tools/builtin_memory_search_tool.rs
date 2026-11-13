use axon_domain::{
    ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    ToolSchemaFieldKind,
};

/// 内置 memory_search tool 名称。
pub const MEMORY_SEARCH_TOOL_NAME: &str = "memory_search";
/// memory_search tool schema version；新增必填 `intent` 后升到 2。
pub const MEMORY_SEARCH_TOOL_SCHEMA_VERSION: u32 = 2;
/// memory_search tool ID。
pub const MEMORY_SEARCH_TOOL_ID: ToolId = ToolId(17);
/// memory_search tool 必填 query 字段。
pub const MEMORY_SEARCH_TOOL_QUERY_FIELD: &str = "query";
/// memory_search tool 可选 limit 字段。
pub const MEMORY_SEARCH_TOOL_LIMIT_FIELD: &str = "limit";
/// memory_search 可选 since_ms 字段。
pub const MEMORY_SEARCH_SINCE_FIELD: &str = "since_ms";
/// memory_search 可选 until_ms 字段。
pub const MEMORY_SEARCH_UNTIL_FIELD: &str = "until_ms";
/// memory_search 默认最大返回条目数。
pub const MEMORY_SEARCH_DEFAULT_LIMIT: usize = 5;

pub(crate) fn memory_search_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        MEMORY_SEARCH_TOOL_SCHEMA_VERSION,
        [MEMORY_SEARCH_TOOL_QUERY_FIELD, crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                crate::TOOL_INTENT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                crate::TOOL_INTENT_FIELD_DESCRIPTION,
            ),
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
            ToolSchemaField::new(
                MEMORY_SEARCH_SINCE_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选时间窗起点，单位毫秒 Unix 时间戳；与 until_ms 成对出现，只返回该区间内更新过的记忆。",
            ),
            ToolSchemaField::new(
                MEMORY_SEARCH_UNTIL_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选时间窗终点，单位毫秒 Unix 时间戳；与 since_ms 成对出现。",
            ),
        ],
    );
    schema.input_schema["properties"][MEMORY_SEARCH_TOOL_LIMIT_FIELD]["minimum"] =
        serde_json::json!(1);
    // 单条完整调用示例：intent 与 query 同批出现，模型不靠猜字段顺序。
    schema.input_schema["examples"] = serde_json::json!([{
        "intent": "回忆既有决策",
        "query": "仓库文档重整的偏好与约束",
        "limit": 5,
    }]);
    ToolDefinition {
        id: MEMORY_SEARCH_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: MEMORY_SEARCH_TOOL_NAME.to_owned(),
        description:
            "当过去的用户偏好、项目决策或经验可能影响当前任务时，描述具体问题并检索相关长期记忆。检索范围是个人记忆加当前工作地点：普通 Workspace（含其分支）或在 ~/.axon 里工作时的 Axon 主目录桶。intent 是每次检索必填的一句话目的，写法见该字段描述；漏填会让整条调用被判为参数非法。"
                .to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}
