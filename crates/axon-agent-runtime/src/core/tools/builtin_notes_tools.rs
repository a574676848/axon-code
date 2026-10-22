//! 内置 notes 命名空间 tool 定义（对齐 Codex history-notes 的 notes 能力）。
//!
//! 便签是私有模型专用状态：用于跨记忆窗口保存目标、决策、进展与下一步。
//! 工具只作用于当前会话，不写长期记忆，也不得向用户披露路径或内容。

use axon_domain::{
    ToolConcurrencyPolicy, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};

use crate::AGENT_TOOL_NAMESPACE;

/// 整文件写入便签的工具名称。
pub const NOTES_WRITE_FILE_TOOL_NAME: &str = "notes_write_file";
/// 整文件写入便签的完整限定名称。
pub const NOTES_WRITE_FILE_TOOL_QUALIFIED_NAME: &str = "builtin.notes_write_file";
/// 追加写入便签的工具名称。
pub const NOTES_APPEND_TO_FILE_TOOL_NAME: &str = "notes_append_to_file";
/// 追加写入便签的完整限定名称。
pub const NOTES_APPEND_TO_FILE_TOOL_QUALIFIED_NAME: &str = "builtin.notes_append_to_file";
/// 读取便签的工具名称。
pub const NOTES_READ_FILE_TOOL_NAME: &str = "notes_read_file";
/// 读取便签的完整限定名称。
pub const NOTES_READ_FILE_TOOL_QUALIFIED_NAME: &str = "builtin.notes_read_file";
/// 按前缀列举便签的工具名称。
pub const NOTES_LIST_FILES_BY_PREFIX_TOOL_NAME: &str = "notes_list_files_by_prefix";
/// 按前缀列举便签的完整限定名称。
pub const NOTES_LIST_FILES_BY_PREFIX_TOOL_QUALIFIED_NAME: &str =
    "builtin.notes_list_files_by_prefix";
/// 检索便签内容的工具名称。
pub const NOTES_SEARCH_CONTENTS_TOOL_NAME: &str = "notes_search_contents";
/// 检索便签内容的完整限定名称。
pub const NOTES_SEARCH_CONTENTS_TOOL_QUALIFIED_NAME: &str = "builtin.notes_search_contents";

/// 便签路径字段。
pub const NOTES_PATH_FIELD: &str = "path";
/// 便签文本字段。
pub const NOTES_TEXT_FIELD: &str = "text";
/// 便签路径前缀字段。
pub const NOTES_PREFIX_FIELD: &str = "prefix";
/// 便签检索关键字字段。
pub const NOTES_QUERY_FIELD: &str = "query";
/// 便签列举最大返回文件数字段。
pub const NOTES_MAX_RESULTS_FIELD: &str = "max_results";
/// 便签列举排序字段。
pub const NOTES_FILE_ORDER_BY_FIELD: &str = "file_order_by";
/// 便签列举排序方向字段。
pub const NOTES_FILE_ORDER_FIELD: &str = "file_order";
/// 便签读取起始行字段。
pub const NOTES_START_LINE_FIELD: &str = "start_line";
/// 便签读取结束行字段。
pub const NOTES_STOP_LINE_FIELD: &str = "stop_line";
/// 便签检索单文件命中行上限字段。
pub const NOTES_MAX_MATCHES_PER_FILE_FIELD: &str = "max_matches_per_file";
/// 便签检索文件数上限字段。
pub const NOTES_MAX_FILES_FIELD: &str = "max_files";
/// 便签检索路径前缀字段。
pub const NOTES_PATH_PREFIX_FIELD: &str = "path_prefix";
/// 便签检索按创建时间倒序字段。
pub const NOTES_RECENT_FILE_FIRST_FIELD: &str = "recent_file_first";

/// 便签按 UTF-8 字节数执行限制；标准 JSON Schema 的 maxLength 按字符计数，
/// 因此同时输出 Axon 扩展元数据供运行时契约识别。
pub const NOTES_UTF8_BYTES_LIMIT_SCHEMA_KEY: &str = "x-axon-max-utf8-bytes";

/// 便签列举排序字段的可用取值。
pub const NOTES_FILE_ORDER_BY_VALUES: [&str; 3] = ["name", "created_at", "updated_at"];
/// 便签列举排序方向的可用取值。
pub const NOTES_FILE_ORDER_VALUES: [&str; 2] = ["ascending", "descending"];

const NOTES_WRITE_FILE_TOOL_ID: ToolId = ToolId(52);
const NOTES_APPEND_TO_FILE_TOOL_ID: ToolId = ToolId(53);
const NOTES_READ_FILE_TOOL_ID: ToolId = ToolId(54);
const NOTES_LIST_FILES_BY_PREFIX_TOOL_ID: ToolId = ToolId(55);
const NOTES_SEARCH_CONTENTS_TOOL_ID: ToolId = ToolId(56);

/// 便签命名空间的共享语义说明（对齐 Codex NOTES_DESCRIPTION 的能力范围与隐私约束）。
const NOTES_PRIVACY_NOTE: &str = "私有便签用于跨记忆窗口保留工作状态。路径是虚拟便签路径而非文件系统路径：相对路径位于当前会话便签目录，例如 checkpoint.md 或 handoff/open-issues.md；不支持绝对路径、空组件、`.`、`..`，也不做 shell 展开（`~` 按字面量处理）。便签直接读写本机文件：读取、列表与检索都立即反映已成功的写入。每个文件不得超过 1,000,000 UTF-8 字节，接近上限时请新建文件。这是私有模型专用状态：静默用于继续任务，不得向用户披露工具、路径、内容或恢复机制。";

/// 便签写入工具 schema 版本；必填 `intent` 后升到 2。
const NOTES_WRITE_TOOL_SCHEMA_VERSION: u32 = 2;

/// 注册到 provider 的 notes 命名空间工具集合。
#[must_use]
pub fn notes_tool_definitions() -> [ToolDefinition; 5] {
    [
        notes_write_file_tool_definition(),
        notes_append_to_file_tool_definition(),
        notes_read_file_tool_definition(),
        notes_list_files_by_prefix_tool_definition(),
        notes_search_contents_tool_definition(),
    ]
}

fn notes_write_file_tool_definition() -> ToolDefinition {
    let text_field = ToolSchemaField::new(
        NOTES_TEXT_FIELD,
        ToolSchemaFieldKind::String,
        "整文件替换文本，按原样写入；执行时按 UTF-8 字节数计量，最多 1,000,000 字节。",
    );
    let mut schema = ToolSchema::object(
        NOTES_WRITE_TOOL_SCHEMA_VERSION,
        [NOTES_PATH_FIELD, NOTES_TEXT_FIELD, crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                NOTES_PATH_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "虚拟便签路径，例如 checkpoint.md 或 handoff/open-issues.md；不支持绝对路径、`..` 与 `\\`。",
            ),
            text_field,
            notes_intent_field(),
        ],
    );
    set_schema_text_limit(&mut schema);
    notes_tool_definition(
        NOTES_WRITE_FILE_TOOL_ID,
        NOTES_WRITE_FILE_TOOL_NAME,
        "创建或整文件替换私有便签；单文件不得超过 1,000,000 UTF-8 字节，接近上限时请新建文件。intent 是每次调用必填的一句话目的，写法见该字段描述。",
        schema,
        ToolPermissionPolicy::GovernedWrite,
        ToolConcurrencyPolicy::Exclusive,
    )
}

fn notes_append_to_file_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        NOTES_WRITE_TOOL_SCHEMA_VERSION,
        [NOTES_PATH_FIELD, NOTES_TEXT_FIELD, crate::TOOL_INTENT_FIELD],
        vec![
            ToolSchemaField::new(
                NOTES_PATH_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "虚拟便签路径；文件不存在时自动创建。",
            ),
            ToolSchemaField::new(
                NOTES_TEXT_FIELD,
                ToolSchemaFieldKind::String,
                "按原样追加到文件末尾的文本；执行时按 UTF-8 字节数计量，追加后文件总大小最多 1,000,000 字节。",
            ),
            notes_intent_field(),
        ],
    );
    set_schema_text_limit(&mut schema);
    notes_tool_definition(
        NOTES_APPEND_TO_FILE_TOOL_ID,
        NOTES_APPEND_TO_FILE_TOOL_NAME,
        "把文本原样追加到私有便签末尾；追加后仍受单文件 1,000,000 UTF-8 字节上限约束。intent 是每次调用必填的一句话目的，写法见该字段描述。",
        schema,
        ToolPermissionPolicy::GovernedWrite,
        ToolConcurrencyPolicy::Exclusive,
    )
}

/// 便签写入工具的必填目的概要字段；只用于界面折叠态展示。
fn notes_intent_field() -> ToolSchemaField {
    ToolSchemaField::new(
        crate::TOOL_INTENT_FIELD,
        ToolSchemaFieldKind::NonBlankString,
        crate::TOOL_INTENT_FIELD_DESCRIPTION,
    )
}

fn notes_read_file_tool_definition() -> ToolDefinition {
    let schema = ToolSchema::object(
        1,
        [NOTES_PATH_FIELD],
        vec![
            ToolSchemaField::new(
                NOTES_PATH_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "要读取的虚拟便签路径。",
            ),
            ToolSchemaField::new(
                NOTES_START_LINE_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选。返回的首行，1-based 且包含该行；负值从文件末尾回数；null 或省略时从第 1 行开始。",
            ),
            ToolSchemaField::new(
                NOTES_STOP_LINE_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选。返回的末行，1-based 且包含该行；负值从文件末尾回数；null 或省略时读到文件末尾。",
            ),
        ],
    );
    notes_tool_definition(
        NOTES_READ_FILE_TOOL_ID,
        NOTES_READ_FILE_TOOL_NAME,
        "读取私有便签的全部或指定行范围。",
        schema,
        ToolPermissionPolicy::ReadOnly,
        ToolConcurrencyPolicy::ParallelSafe,
    )
}

fn notes_list_files_by_prefix_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        1,
        std::iter::empty::<&str>(),
        vec![
            ToolSchemaField::new(
                NOTES_PREFIX_FIELD,
                ToolSchemaFieldKind::String,
                "可选。便签虚拟路径前缀；省略或传 null 时列举当前会话的全部便签。",
            ),
            ToolSchemaField::new(
                NOTES_MAX_RESULTS_FIELD,
                ToolSchemaFieldKind::Integer,
                format!(
                    "可选。最多返回的文件数，必须为正整数，默认 {}，最大 {}；超过最大值直接失败。",
                    crate::session_notes_store::NOTES_LIST_MAX_RESULTS,
                    crate::session_notes_store::NOTES_LIST_MAX_RESULTS
                ),
            ),
            ToolSchemaField::new(
                NOTES_FILE_ORDER_BY_FIELD,
                ToolSchemaFieldKind::StringEnum(&NOTES_FILE_ORDER_BY_VALUES),
                "可选。排序字段，取值为 name、created_at 或 updated_at，默认 updated_at。",
            ),
            ToolSchemaField::new(
                NOTES_FILE_ORDER_FIELD,
                ToolSchemaFieldKind::StringEnum(&NOTES_FILE_ORDER_VALUES),
                "可选。排序方向，取值为 ascending 或 descending，默认 descending。",
            ),
        ],
    );
    schema.input_schema["properties"][NOTES_MAX_RESULTS_FIELD]["minimum"] = serde_json::json!(1);
    schema.input_schema["properties"][NOTES_MAX_RESULTS_FIELD]["maximum"] =
        serde_json::json!(crate::session_notes_store::NOTES_LIST_MAX_RESULTS);
    notes_tool_definition(
        NOTES_LIST_FILES_BY_PREFIX_TOOL_ID,
        NOTES_LIST_FILES_BY_PREFIX_TOOL_NAME,
        "按路径前缀列举私有便签文件及其大小与时间戳。",
        schema,
        ToolPermissionPolicy::ReadOnly,
        ToolConcurrencyPolicy::ParallelSafe,
    )
}

fn notes_search_contents_tool_definition() -> ToolDefinition {
    let mut schema = ToolSchema::object(
        1,
        [NOTES_QUERY_FIELD],
        vec![
            ToolSchemaField::new(
                NOTES_QUERY_FIELD,
                ToolSchemaFieldKind::NonBlankString,
                "区分大小写的字面量，用于在便签行内查找。",
            ),
            ToolSchemaField::new(
                NOTES_MAX_MATCHES_PER_FILE_FIELD,
                ToolSchemaFieldKind::Integer,
                "可选。每个文件最多返回的命中行数，必须为正整数，默认 1。",
            ),
            ToolSchemaField::new(
                NOTES_MAX_FILES_FIELD,
                ToolSchemaFieldKind::Integer,
                format!(
                    "可选。最多返回的命中文件数，必须为正整数，默认 {}，最大 {}；超过最大值直接失败。",
                    crate::session_notes_store::NOTES_LIST_MAX_RESULTS,
                    crate::session_notes_store::NOTES_LIST_MAX_RESULTS
                ),
            ),
            ToolSchemaField::new(
                NOTES_PATH_PREFIX_FIELD,
                ToolSchemaFieldKind::String,
                "可选。限定检索的便签路径前缀；省略或传 null 时检索当前会话全部便签。",
            ),
            ToolSchemaField::new(
                NOTES_RECENT_FILE_FIRST_FIELD,
                ToolSchemaFieldKind::Boolean,
                "可选。为 true 时按文件创建时间从新到旧返回文件，默认 false。",
            ),
        ],
    );
    for field in [NOTES_MAX_MATCHES_PER_FILE_FIELD, NOTES_MAX_FILES_FIELD] {
        schema.input_schema["properties"][field]["minimum"] = serde_json::json!(1);
    }
    schema.input_schema["properties"][NOTES_MAX_FILES_FIELD]["maximum"] =
        serde_json::json!(crate::session_notes_store::NOTES_LIST_MAX_RESULTS);
    notes_tool_definition(
        NOTES_SEARCH_CONTENTS_TOOL_ID,
        NOTES_SEARCH_CONTENTS_TOOL_NAME,
        "在私有便签内做区分大小写的字面量检索，返回命中文件与命中行。",
        schema,
        ToolPermissionPolicy::ReadOnly,
        ToolConcurrencyPolicy::ParallelSafe,
    )
}

fn notes_tool_definition(
    id: ToolId,
    name: &str,
    description: &str,
    mut schema: ToolSchema,
    permission_policy: ToolPermissionPolicy,
    concurrency_policy: ToolConcurrencyPolicy,
) -> ToolDefinition {
    remove_schema_minimum(&mut schema, NOTES_START_LINE_FIELD);
    remove_schema_minimum(&mut schema, NOTES_STOP_LINE_FIELD);
    set_schema_nullable(
        &mut schema,
        &[
            NOTES_PREFIX_FIELD,
            NOTES_START_LINE_FIELD,
            NOTES_STOP_LINE_FIELD,
            NOTES_PATH_PREFIX_FIELD,
        ],
    );
    set_schema_max_length(
        &mut schema,
        NOTES_PATH_FIELD,
        crate::session_notes_store::NOTES_MAX_PATH_CHARS,
    );
    set_schema_max_length(
        &mut schema,
        NOTES_PREFIX_FIELD,
        crate::session_notes_store::NOTES_MAX_PATH_CHARS,
    );
    set_schema_max_length(
        &mut schema,
        NOTES_PATH_PREFIX_FIELD,
        crate::session_notes_store::NOTES_MAX_PATH_CHARS,
    );
    ToolDefinition {
        id,
        namespace: AGENT_TOOL_NAMESPACE.to_owned(),
        name: name.to_owned(),
        description: format!("{description}{NOTES_PRIVACY_NOTE}"),
        schema,
        enabled: true,
        concurrency_policy,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy,
    }
}

/// 为可选字段保留原 schema，并增加 Codex 兼容的 null 分支。
fn set_schema_nullable(schema: &mut ToolSchema, fields: &[&str]) {
    let Some(properties) = schema.input_schema.get_mut("properties") else {
        return;
    };
    for field in fields {
        let Some(property) = properties.get_mut(*field) else {
            continue;
        };
        let original = std::mem::take(property);
        let description = original.get("description").cloned();
        let mut nullable = serde_json::json!({
            "anyOf": [original, {"type": "null"}],
        });
        if let Some(description) = description {
            nullable["description"] = description;
        }
        *property = nullable;
    }
}

/// 为字段增加字符上限；nullable 字段只约束 string 分支。
fn set_schema_max_length(schema: &mut ToolSchema, field: &str, maximum: usize) {
    let Some(property) = schema
        .input_schema
        .get_mut("properties")
        .and_then(|properties| properties.get_mut(field))
    else {
        return;
    };
    if let Some(branches) = property
        .get_mut("anyOf")
        .and_then(serde_json::Value::as_array_mut)
    {
        for branch in branches {
            if branch["type"] == "string" {
                branch["maxLength"] = serde_json::json!(maximum);
            }
        }
    } else {
        property["maxLength"] = serde_json::json!(maximum);
    }
}

/// 仅声明 Axon 精确 UTF-8 字节上限；标准 JSON Schema 没有 UTF-8 字节约束。
fn set_schema_text_limit(schema: &mut ToolSchema) {
    let Some(property) = schema
        .input_schema
        .get_mut("properties")
        .and_then(|properties| properties.get_mut(NOTES_TEXT_FIELD))
    else {
        return;
    };
    property
        .as_object_mut()
        .expect("文本字段 schema 必须是对象")
        .remove("maxLength");
    property[NOTES_UTF8_BYTES_LIMIT_SCHEMA_KEY] =
        serde_json::json!(crate::session_notes_store::NOTES_MAX_FILE_BYTES);
}

/// 删除字段构造器提供的默认非负下界。
fn remove_schema_minimum(schema: &mut ToolSchema, field: &str) {
    let Some(property) = schema
        .input_schema
        .get_mut("properties")
        .and_then(|properties| properties.get_mut(field))
    else {
        return;
    };
    if let Some(object) = property.as_object_mut() {
        object.remove("minimum");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    fn property_names(definition: &ToolDefinition) -> BTreeSet<String> {
        definition
            .schema
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn name_set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    /// 便签字段集与 Codex 对齐，另加 Axon 侧的必填展示字段 `intent`。
    #[test]
    fn notes_tool_schemas_match_codex_field_sets() {
        let expected: [(&str, &[&str], &[&str]); 5] = [
            (
                NOTES_WRITE_FILE_TOOL_NAME,
                &["intent", "path", "text"],
                &["intent", "path", "text"],
            ),
            (
                NOTES_APPEND_TO_FILE_TOOL_NAME,
                &["intent", "path", "text"],
                &["intent", "path", "text"],
            ),
            (
                NOTES_READ_FILE_TOOL_NAME,
                &["path", "start_line", "stop_line"],
                &["path"],
            ),
            (
                NOTES_LIST_FILES_BY_PREFIX_TOOL_NAME,
                &["file_order", "file_order_by", "max_results", "prefix"],
                &[],
            ),
            (
                NOTES_SEARCH_CONTENTS_TOOL_NAME,
                &[
                    "max_files",
                    "max_matches_per_file",
                    "path_prefix",
                    "query",
                    "recent_file_first",
                ],
                &["query"],
            ),
        ];
        let definitions = notes_tool_definitions();
        for (name, properties, required) in expected {
            let definition = definitions
                .iter()
                .find(|definition| definition.name == name)
                .unwrap_or_else(|| panic!("缺少 notes 工具定义：{name}"));
            assert_eq!(
                property_names(definition),
                name_set(properties),
                "{name} 的属性集必须与 Codex 字段集一致（另加 Axon 必填 intent）"
            );
            let required_fields = definition
                .schema
                .required_field_names()
                .into_iter()
                .collect::<BTreeSet<_>>();
            assert_eq!(
                required_fields,
                name_set(required),
                "{name} 的必填字段必须与 Codex 字段集一致（另加 Axon 必填 intent）"
            );
        }
    }

    #[test]
    fn notes_tool_descriptions_are_product_ready_and_self_consistent() {
        for definition in notes_tool_definitions() {
            let description = definition.description.as_str();
            assert!(
                !description.contains('{') && !description.contains('}'),
                "{} 的描述不得残留模板占位符",
                definition.name
            );
            assert!(
                !description.contains("最终一致"),
                "{} 的描述不得声明最终一致：便签读写本机文件，立即可见",
                definition.name
            );
            assert!(
                description.contains("立即反映已成功的写入"),
                "{} 的描述必须声明本地立即可见语义",
                definition.name
            );
            for property in property_names(&definition) {
                let field_schema = &definition.schema.input_schema["properties"][&property];
                assert!(
                    field_schema.is_object(),
                    "{} 的 {property} 必须带有字段级 schema",
                    definition.name
                );
                let field_description = field_schema["description"].as_str().unwrap_or_default();
                assert!(
                    !field_description.trim().is_empty(),
                    "{} 的 {property} 字段缺少 description",
                    definition.name
                );
                assert!(
                    !field_description.contains('{') && !field_description.contains('}'),
                    "{} 的 {property} 字段描述不得残留模板占位符",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn notes_optional_fields_match_codex_nullable_and_line_contract() {
        let definitions = notes_tool_definitions();
        for definition in definitions {
            let properties = &definition.schema.input_schema["properties"];
            for field in [
                NOTES_PREFIX_FIELD,
                NOTES_START_LINE_FIELD,
                NOTES_STOP_LINE_FIELD,
                NOTES_PATH_PREFIX_FIELD,
            ] {
                let Some(property) = properties.get(field) else {
                    continue;
                };
                assert!(
                    property["anyOf"].as_array().is_some_and(|branches| branches
                        .iter()
                        .any(|branch| branch["type"] == "null")),
                    "{} 的 {field} 必须允许 null",
                    definition.name
                );
            }
        }

        let read = notes_tool_definitions()
            .into_iter()
            .find(|definition| definition.name == NOTES_READ_FILE_TOOL_NAME)
            .expect("必须存在 notes_read_file");
        for field in [NOTES_START_LINE_FIELD, NOTES_STOP_LINE_FIELD] {
            assert!(
                read.schema.input_schema["properties"][field]["anyOf"][0]
                    .get("minimum")
                    .is_none(),
                "{field} 必须允许负行号"
            );
        }

        for name in [NOTES_WRITE_FILE_TOOL_NAME, NOTES_APPEND_TO_FILE_TOOL_NAME] {
            let definition = notes_tool_definitions()
                .into_iter()
                .find(|definition| definition.name == name)
                .expect("必须存在 notes 写入工具");
            let text_schema = &definition.schema.input_schema["properties"][NOTES_TEXT_FIELD];
            assert!(
                text_schema.get("maxLength").is_none(),
                "UTF-8 字节上限不能伪装为按字符计量的 maxLength"
            );
            assert_eq!(
                text_schema[NOTES_UTF8_BYTES_LIMIT_SCHEMA_KEY],
                serde_json::json!(crate::session_notes_store::NOTES_MAX_FILE_BYTES)
            );
        }
    }
}
