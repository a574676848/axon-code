use axon_domain::{
    ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    ToolSchemaFieldKind,
};

/// 内置 memory_write tool 名称。
pub const MEMORY_WRITE_TOOL_NAME: &str = "memory_write";
/// memory_write tool schema version。
pub const MEMORY_WRITE_TOOL_SCHEMA_VERSION: u32 = 5;
/// memory_write tool ID。
pub const MEMORY_WRITE_TOOL_ID: ToolId = ToolId(19);
/// memory_write 记忆类型字段。
pub const MEMORY_WRITE_TOOL_KIND_FIELD: &str = "kind";
/// memory_write 稳定结论字段。
pub const MEMORY_WRITE_TOOL_SUMMARY_FIELD: &str = "summary";
/// memory_write 验证证据字段。
pub const MEMORY_WRITE_TOOL_EVIDENCE_FIELD: &str = "evidence";
/// memory_write 被替代 Memory ID 字段。
pub const MEMORY_WRITE_TOOL_SUPERSEDES_IDS_FIELD: &str = "supersedes_memory_ids";
/// memory_write 支持的通用记忆类型。
pub const MEMORY_WRITE_TOOL_KINDS: &[&str] = &[
    "profile",
    "preference",
    "experience",
    "knowledge",
    "procedure",
];

pub(crate) fn memory_write_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        MEMORY_WRITE_TOOL_SCHEMA_VERSION,
        [
            MEMORY_WRITE_TOOL_KIND_FIELD,
            MEMORY_WRITE_TOOL_SUMMARY_FIELD,
            MEMORY_WRITE_TOOL_EVIDENCE_FIELD,
        ],
        vec![
            ToolSchemaField::new(
                MEMORY_WRITE_TOOL_KIND_FIELD,
                ToolSchemaFieldKind::StringEnum(MEMORY_WRITE_TOOL_KINDS),
                "记忆类型：profile、preference、experience、knowledge 或 procedure。",
            ),
            ToolSchemaField::new(
                MEMORY_WRITE_TOOL_SUMMARY_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "非空、可跨会话复用的记忆正文；用户授权保存时可包含账号、密码、token 等敏感明文。",
            ),
            ToolSchemaField::new(
                MEMORY_WRITE_TOOL_EVIDENCE_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "非空支撑证据：用户明确表达、当前事实或可审计验证摘要。",
            ),
            ToolSchemaField::new(
                MEMORY_WRITE_TOOL_SUPERSEDES_IDS_FIELD,
                ToolSchemaFieldKind::IntegerArray,
                "可选单个正整数或十进制字符串 ID，或对应数组；大于 Provider 安全整数范围的 ID 必须使用十进制字符串；仅当新结论明确使已召回的旧记忆失效或错误时填写。",
            ),
        ],
    );
    schema.input_schema["properties"][MEMORY_WRITE_TOOL_SUPERSEDES_IDS_FIELD] = serde_json::json!({
        "anyOf": [
            {
                "type": "string",
                "pattern": "^[1-9][0-9]*$"
            },
            {
                "type": "array",
                "items": {
                    "anyOf": [
                        {
                            "type": "string",
                            "pattern": "^[1-9][0-9]*$"
                        },
                        { "type": "integer", "minimum": 1 }
                    ]
                },
                "uniqueItems": true
            },
            { "type": "integer", "minimum": 1 }
        ],
        "description": "可选单个正整数或十进制字符串 ID，或对应数组；大于 Provider 安全整数范围的 ID 必须使用十进制字符串。仅当新结论明确使已召回的旧记忆失效或错误时填写，相关但不矛盾的记忆不得替代。",
        "examples": ["8079221640628927909", [42, "8079221640628927909"]]
    });
    ToolDefinition {
        id: MEMORY_WRITE_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: MEMORY_WRITE_TOOL_NAME.to_owned(),
        description: "仅在当前任务得到有证据、可跨会话复用的稳定结论时写入长期记忆。作用域固定：profile 与 preference 写入个人记忆；experience、knowledge 与 procedure 写入当前工作地点（普通 Workspace 或其分支，以及在 ~/.axon 里写作时固定的 Axon 主目录桶），不得因为位于主目录就降级成个人记忆。用户授权保存的敏感信息按明文进入普通 Memory，不提供加密隔离。".to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::GovernedWrite,
    }
}
