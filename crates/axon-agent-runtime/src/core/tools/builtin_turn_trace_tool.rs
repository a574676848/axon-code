use axon_domain::{
    ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    ToolSchemaFieldKind,
};

/// 内置 turn_trace tool 名称。
pub const TURN_TRACE_TOOL_NAME: &str = "turn_trace";
/// turn_trace tool schema version。
pub const TURN_TRACE_TOOL_SCHEMA_VERSION: u32 = 1;
/// turn_trace tool ID。
pub const TURN_TRACE_TOOL_ID: ToolId = ToolId(25);
/// 精确回取单条回执的字段。
pub const TURN_TRACE_RECEIPT_REF_FIELD: &str = "receipt_ref";
/// 按轨迹取整轮切片的字段。
pub const TURN_TRACE_TRACE_ID_FIELD: &str = "trace_id";
/// 按会话定位的字段。
pub const TURN_TRACE_SESSION_ID_FIELD: &str = "session_id";
/// 按轮序定位的字段。
pub const TURN_TRACE_SEQUENCE_FIELD: &str = "sequence";
/// 单次返回上限字段。
pub const TURN_TRACE_MAX_CHARS_FIELD: &str = "max_chars";
/// 单次返回默认上限，单位字符。
pub const TURN_TRACE_DEFAULT_MAX_CHARS: u64 = 8_000;
/// 单次返回硬上限，单位字符。
pub const TURN_TRACE_MAX_CHARS_LIMIT: u64 = 32_000;

pub(crate) fn turn_trace_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        TURN_TRACE_TOOL_SCHEMA_VERSION,
        [crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                crate::TOOL_INTENT_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                crate::TOOL_INTENT_FIELD_DESCRIPTION,
            ),
            ToolSchemaField::new(
                TURN_TRACE_RECEIPT_REF_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "精确回取单条工具回执的引用，与 trace_id、session_id 加 sequence 三选一。最省，优先用它。",
            ),
            ToolSchemaField::new(
                TURN_TRACE_TRACE_ID_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "按轨迹 ID 取整轮的工具回执切片，与 receipt_ref、session_id 加 sequence 三选一。",
            ),
            ToolSchemaField::new(
                TURN_TRACE_SESSION_ID_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "按会话 ID 定位，必须与 sequence 同时给出；与 receipt_ref、trace_id 三选一。",
            ),
            ToolSchemaField::new(
                TURN_TRACE_SEQUENCE_FIELD,
                ToolSchemaFieldKind::Integer,
                "轮次序号，从 0 开始，必须与 session_id 同时给出。",
            ),
            ToolSchemaField::new(
                TURN_TRACE_MAX_CHARS_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选单次返回上限，单位字符，默认 8000，最大 32000。超出上限时如实上报截断与总量。",
            ),
        ],
    );
    schema.input_schema["properties"][TURN_TRACE_SEQUENCE_FIELD]["minimum"] = serde_json::json!(0);
    schema.input_schema["properties"][TURN_TRACE_MAX_CHARS_FIELD]["minimum"] = serde_json::json!(1);
    schema.input_schema["properties"][TURN_TRACE_MAX_CHARS_FIELD]["maximum"] =
        serde_json::json!(TURN_TRACE_MAX_CHARS_LIMIT);
    schema.input_schema["examples"] = serde_json::json!([
        {
            "intent": "核对构建命令的实际输出",
            "receipt_ref": "receipt:3",
        },
        {
            "intent": "回看这一轮的全部工具执行",
            "trace_id": "trace:7:18",
            "max_chars": 8000,
        }
    ]);
    ToolDefinition {
        id: TURN_TRACE_TOOL_ID,
        namespace: crate::AGENT_TOOL_NAMESPACE.to_owned(),
        name: TURN_TRACE_TOOL_NAME.to_owned(),
        description:
            "按引用回取单轮工具回执的原文，用来核对这一轮到底执行了什么。轮次记录只保存摘要与 outputRef 指针，完整原文在归档文件里，本工具负责按引用定位并返回，因此不受轮次摘要长度限制。三种寻址按优先级 receipt_ref、trace_id、session_id 加 sequence 取其一，同时给出多组时只取优先级最高的一组，不合并。跨 owner 或跨会话的引用一律拒绝；引用不存在或已超出保留窗口时明确报错，不返回空数组冒充成功。intent 是每次调用必填的一句话目的，写法见该字段描述；漏填会让整条调用被判为参数非法。"
                .to_owned(),
        schema,
        enabled: true,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_requires_intent_and_declares_every_address_field() {
        let definition = turn_trace_tool_definition();
        assert_eq!(definition.name, TURN_TRACE_TOOL_NAME);
        assert_eq!(definition.id, TURN_TRACE_TOOL_ID);

        let required = definition.schema.input_schema["required"]
            .as_array()
            .expect("required 必须是数组");
        assert_eq!(required.len(), 1, "只有 intent 是 schema 级必填");
        assert_eq!(required[0], crate::TOOL_INTENT_FIELD);

        let properties = definition.schema.input_schema["properties"]
            .as_object()
            .expect("properties 必须是对象");
        for field in [
            TURN_TRACE_RECEIPT_REF_FIELD,
            TURN_TRACE_TRACE_ID_FIELD,
            TURN_TRACE_SESSION_ID_FIELD,
            TURN_TRACE_SEQUENCE_FIELD,
            TURN_TRACE_MAX_CHARS_FIELD,
        ] {
            assert!(properties.contains_key(field), "缺少寻址或上限字段 {field}");
        }
    }

    #[test]
    fn schema_bounds_sequence_and_max_chars_and_examples_carry_intent() {
        let schema = turn_trace_tool_definition().schema;
        assert_eq!(
            schema.input_schema["properties"][TURN_TRACE_SEQUENCE_FIELD]["minimum"],
            serde_json::json!(0)
        );
        assert_eq!(
            schema.input_schema["properties"][TURN_TRACE_MAX_CHARS_FIELD]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            schema.input_schema["properties"][TURN_TRACE_MAX_CHARS_FIELD]["maximum"],
            serde_json::json!(TURN_TRACE_MAX_CHARS_LIMIT)
        );

        let examples = schema.input_schema["examples"]
            .as_array()
            .expect("examples 必须是数组");
        assert_eq!(examples.len(), 2);
        for example in examples {
            assert!(
                example.get(crate::TOOL_INTENT_FIELD).is_some(),
                "每个示例都必须带 intent"
            );
        }
    }

    #[test]
    fn tool_is_read_only_parallel_safe_and_bounded() {
        let definition = turn_trace_tool_definition();
        assert_eq!(definition.permission_policy, ToolPermissionPolicy::ReadOnly);
        assert_eq!(
            definition.concurrency_policy,
            axon_domain::ToolConcurrencyPolicy::ParallelSafe
        );
        assert_eq!(definition.output_policy, ToolOutputPolicy::BoundedSummary);
        assert!(
            definition.description.contains("intent"),
            "description 必须说明 intent 必填"
        );
        assert!(
            definition.description.contains("三种寻址"),
            "description 必须说明三种寻址与优先级"
        );
    }
}
