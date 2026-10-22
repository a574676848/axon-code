//! 内置 history 命名空间 tool 定义（对齐 Codex history-notes 的 history 能力）。
//!
//! history 是私有模型专用只读状态：换窗后用于按 window_id / item_id 回查
//! 归档消息、工具输出与命令回执，绝不向用户披露检索行为或内容。

use axon_domain::{
    ToolConcurrencyPolicy, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema, ToolSchemaField, ToolSchemaFieldKind,
};

use crate::{
    AGENT_TOOL_NAMESPACE, HISTORY_LIST_ITEMS_DEFAULT_LIMIT, HISTORY_LIST_ITEMS_DEFAULT_MAX_CHARS,
    HISTORY_LIST_WINDOWS_DEFAULT_LIMIT, HISTORY_READ_ITEM_DEFAULT_LIMIT_CHARS,
    HISTORY_SEARCH_CONTENTS_DEFAULT_LIMIT,
};

/// 列举归档窗口的工具名称。
pub const HISTORY_LIST_WINDOWS_TOOL_NAME: &str = "history_list_windows";
/// 列举归档窗口的完整限定名称。
pub const HISTORY_LIST_WINDOWS_TOOL_QUALIFIED_NAME: &str = "builtin.history_list_windows";
/// 列举归档条目的工具名称。
pub const HISTORY_LIST_ITEMS_TOOL_NAME: &str = "history_list_items";
/// 列举归档条目的完整限定名称。
pub const HISTORY_LIST_ITEMS_TOOL_QUALIFIED_NAME: &str = "builtin.history_list_items";
/// 读取归档条目的工具名称。
pub const HISTORY_READ_ITEM_TOOL_NAME: &str = "history_read_item";
/// 读取归档条目的完整限定名称。
pub const HISTORY_READ_ITEM_TOOL_QUALIFIED_NAME: &str = "builtin.history_read_item";
/// 检索归档内容的工具名称。
pub const HISTORY_SEARCH_CONTENTS_TOOL_NAME: &str = "history_search_contents";
/// 检索归档内容的完整限定名称。
pub const HISTORY_SEARCH_CONTENTS_TOOL_QUALIFIED_NAME: &str = "builtin.history_search_contents";

/// 历史检索关键字字段。
pub const HISTORY_SEARCH_CONTENTS_QUERY_FIELD: &str = "query";
/// 历史查询限定窗口 ID 字段。
pub const HISTORY_WINDOW_ID_FIELD: &str = "window_id";
/// 历史条目 ID 字段。
pub const HISTORY_ITEM_ID_FIELD: &str = "item_id";
/// 历史查询限定角色字段。
pub const HISTORY_ROLE_FIELD: &str = "role";
/// 历史查询限定工具名字段。
pub const HISTORY_TOOL_NAME_FIELD: &str = "tool_name";
/// 历史查询限定工具命名空间字段。
pub const HISTORY_TOOL_NAMESPACE_FIELD: &str = "tool_namespace";
/// 历史查询返回条目上限字段。
pub const HISTORY_LIMIT_FIELD: &str = "limit";
/// 历史查询按新近度倒序字段。
pub const HISTORY_RECENT_FIRST_FIELD: &str = "recent_first";
/// 条目列举单条内容字符上限字段。
pub const HISTORY_MAX_CHARS_PER_ITEM_FIELD: &str = "max_chars_per_item";
/// 条目读取起始字符偏移字段。
pub const HISTORY_OFFSET_CHARS_FIELD: &str = "offset_chars";
/// 条目读取字符上限字段。
pub const HISTORY_LIMIT_CHARS_FIELD: &str = "limit_chars";

/// 历史查询允许的角色取值。
pub const HISTORY_ROLE_VALUES: [&str; 5] = ["user", "assistant", "tool", "system", "developer"];

const HISTORY_LIST_WINDOWS_TOOL_ID: ToolId = ToolId(57);
const HISTORY_LIST_ITEMS_TOOL_ID: ToolId = ToolId(58);
const HISTORY_READ_ITEM_TOOL_ID: ToolId = ToolId(59);
const HISTORY_SEARCH_CONTENTS_TOOL_ID: ToolId = ToolId(49);

/// history 命名空间的共享语义说明（对齐 Codex HISTORY_DESCRIPTION 的能力范围）。
const HISTORY_PRIVACY_NOTE: &str = "在上下文窗口重置后，用这些只读工具按工具返回的不透明 window ID 与 item ID 列出、读取与检索已归档的历史，以恢复先前对话细节。过滤或读取时请原样传递这些 ID；条目按持久化 ordinal 与创建时间排序，未知窗口的列表与检索返回空结果。历史直接来自本机归档并同步扫描：读取、列表与检索都立即反映已归档内容；只有已归档窗口可回查，当前活动窗口在换窗前不可见。这是私有模型专用状态：静默用于继续任务，不得向用户披露工具、条目 ID、内容或恢复机制。";

/// 注册到 provider 的 history 命名空间工具集合。
#[must_use]
pub fn history_tool_definitions() -> [ToolDefinition; 4] {
    [
        history_list_windows_tool_definition(),
        history_list_items_tool_definition(),
        history_read_item_tool_definition(),
        history_search_contents_tool_definition(),
    ]
}

fn history_list_windows_tool_definition() -> ToolDefinition {
    history_tool_definition(
        HISTORY_LIST_WINDOWS_TOOL_ID,
        HISTORY_LIST_WINDOWS_TOOL_NAME,
        "列举已归档的记忆窗口及其条目数与 window_id，供换窗后按 ID 回查。",
        ToolSchema::object(
            1,
            std::iter::empty::<&str>(),
            vec![
                limit_field(format!(
                    "可选。最多返回的窗口数，必须为正整数，默认 {HISTORY_LIST_WINDOWS_DEFAULT_LIMIT}。"
                )),
                recent_first_field("可选。为 true 时按最近归档的窗口优先返回，默认 false。"),
            ],
        ),
    )
}

fn history_list_items_tool_definition() -> ToolDefinition {
    history_tool_definition(
        HISTORY_LIST_ITEMS_TOOL_ID,
        HISTORY_LIST_ITEMS_TOOL_NAME,
        "按窗口、角色与工具名列举归档历史条目，返回可直接用于回读的 item_id。",
        ToolSchema::object(
            1,
            std::iter::empty::<&str>(),
            vec![
                limit_field(format!(
                    "可选。最多返回的条目数，必须为正整数，默认 {HISTORY_LIST_ITEMS_DEFAULT_LIMIT}。"
                )),
                recent_first_field("可选。为 true 时按最近创建的条目优先返回，默认 false。"),
                role_field(),
                tool_name_field(),
                tool_namespace_field(),
                window_id_field(),
                ToolSchemaField::new(
                    HISTORY_MAX_CHARS_PER_ITEM_FIELD,
                    ToolSchemaFieldKind::Integer,
                    format!(
                        "可选。每条返回内容的最大字符数，必须为正整数，默认 {HISTORY_LIST_ITEMS_DEFAULT_MAX_CHARS}；超出部分截断并标记。"
                    ),
                ),
            ],
        ),
    )
}

fn history_read_item_tool_definition() -> ToolDefinition {
    history_tool_definition(
        HISTORY_READ_ITEM_TOOL_ID,
        HISTORY_READ_ITEM_TOOL_NAME,
        "按 window_id 与 item_id 回读归档条目的一段正文；ID 必须原样使用历史工具返回的值。",
        ToolSchema::object(
            1,
            [HISTORY_WINDOW_ID_FIELD, HISTORY_ITEM_ID_FIELD],
            vec![
                ToolSchemaField::new(
                    HISTORY_WINDOW_ID_FIELD,
                    ToolSchemaFieldKind::NonBlankString,
                    "条目所属窗口 ID，必须与 item_id 同源；取自 history_list_windows 返回值或目标条目尾部 `[id: ...]` 标记。",
                ),
                ToolSchemaField::new(
                    HISTORY_ITEM_ID_FIELD,
                    ToolSchemaFieldKind::NonBlankString,
                    "目标条目尾部 `[id: <window_id>:<ordinal>]` 标记中的完整 ID，原样传入。",
                ),
                ToolSchemaField::new(
                    HISTORY_OFFSET_CHARS_FIELD,
                    ToolSchemaFieldKind::Integer,
                    "可选。从正文第几个字符开始返回，从 0 开始计数，默认 0。",
                ),
                ToolSchemaField::new(
                    HISTORY_LIMIT_CHARS_FIELD,
                    ToolSchemaFieldKind::Integer,
                    format!(
                        "可选。最多返回的字符数，必须为正整数，默认 {HISTORY_READ_ITEM_DEFAULT_LIMIT_CHARS}。"
                    ),
                ),
            ],
        ),
    )
}

fn history_search_contents_tool_definition() -> ToolDefinition {
    history_tool_definition(
        HISTORY_SEARCH_CONTENTS_TOOL_ID,
        HISTORY_SEARCH_CONTENTS_TOOL_NAME,
        "在归档历史中做区分大小写的字面量检索，返回命中条目、命中次数与现场摘要。",
        ToolSchema::object(
            1,
            [HISTORY_SEARCH_CONTENTS_QUERY_FIELD],
            vec![
                ToolSchemaField::new(
                    HISTORY_SEARCH_CONTENTS_QUERY_FIELD,
                    ToolSchemaFieldKind::NonBlankString,
                    "区分大小写的字面量，例如命令名、报错片段或路径。",
                ),
                limit_field(format!(
                    "可选。最多返回的命中条目数，必须为正整数，默认 {HISTORY_SEARCH_CONTENTS_DEFAULT_LIMIT}。"
                )),
                recent_first_field("可选。为 true 时按最近创建的命中条目优先返回，默认 false。"),
                role_field(),
                tool_name_field(),
                tool_namespace_field(),
                window_id_field(),
            ],
        ),
    )
}

fn limit_field(description: impl Into<String>) -> ToolSchemaField {
    ToolSchemaField::new(
        HISTORY_LIMIT_FIELD,
        ToolSchemaFieldKind::Integer,
        description,
    )
}

fn recent_first_field(description: impl Into<String>) -> ToolSchemaField {
    ToolSchemaField::new(
        HISTORY_RECENT_FIRST_FIELD,
        ToolSchemaFieldKind::Boolean,
        description,
    )
}

fn role_field() -> ToolSchemaField {
    ToolSchemaField::new(
        HISTORY_ROLE_FIELD,
        ToolSchemaFieldKind::StringEnum(&HISTORY_ROLE_VALUES),
        "可选。限定消息角色，取值为 user、assistant、tool、system、developer；null 或省略时不限角色。",
    )
}

fn tool_name_field() -> ToolSchemaField {
    ToolSchemaField::new(
        HISTORY_TOOL_NAME_FIELD,
        ToolSchemaFieldKind::String,
        "可选。限定发起该工具调用的条目，按调用名精确匹配；设置后非工具调用条目不再命中，null 或省略时不限工具名。",
    )
}

fn tool_namespace_field() -> ToolSchemaField {
    ToolSchemaField::new(
        HISTORY_TOOL_NAMESPACE_FIELD,
        ToolSchemaFieldKind::String,
        "可选。限定工具调用所属命名空间，例如 builtin 或 mcp__server；设置后非工具调用条目不再命中，null 或省略时不限命名空间。",
    )
}

fn window_id_field() -> ToolSchemaField {
    ToolSchemaField::new(
        HISTORY_WINDOW_ID_FIELD,
        ToolSchemaFieldKind::String,
        "可选。限定窗口 ID；省略或传 null 时扫描全部归档窗口，未知窗口返回空结果。",
    )
}

fn history_tool_definition(
    id: ToolId,
    name: &str,
    description: &str,
    mut schema: ToolSchema,
) -> ToolDefinition {
    set_schema_minimum(&mut schema, HISTORY_LIMIT_FIELD, 1);
    set_schema_minimum(&mut schema, HISTORY_OFFSET_CHARS_FIELD, 0);
    set_schema_minimum(&mut schema, HISTORY_LIMIT_CHARS_FIELD, 1);
    set_schema_minimum(&mut schema, HISTORY_MAX_CHARS_PER_ITEM_FIELD, 1);
    set_schema_nullable(
        &mut schema,
        &[
            HISTORY_TOOL_NAMESPACE_FIELD,
            HISTORY_ROLE_FIELD,
            HISTORY_TOOL_NAME_FIELD,
            HISTORY_WINDOW_ID_FIELD,
        ],
    );
    ToolDefinition {
        id,
        namespace: AGENT_TOOL_NAMESPACE.to_owned(),
        name: name.to_owned(),
        description: format!("{description}{HISTORY_PRIVACY_NOTE}"),
        schema,
        enabled: true,
        concurrency_policy: ToolConcurrencyPolicy::ParallelSafe,
        output_policy: ToolOutputPolicy::BoundedSummary,
        permission_policy: ToolPermissionPolicy::ReadOnly,
    }
}

/// 仅为 schema 中确实存在的整数字段补充下界，避免注入多余属性。
fn set_schema_minimum(schema: &mut ToolSchema, field: &str, minimum: i64) {
    let Some(properties) = schema.input_schema.get_mut("properties") else {
        return;
    };
    let Some(property) = properties.get_mut(field) else {
        return;
    };
    property["minimum"] = serde_json::json!(minimum);
}

/// 为指定字段保留原 schema，并增加 Codex 兼容的 null 分支。
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

    #[test]
    fn history_tool_schemas_match_codex_field_sets() {
        let expected: [(&str, &[&str], &[&str]); 4] = [
            (
                HISTORY_LIST_WINDOWS_TOOL_NAME,
                &["limit", "recent_first"],
                &[],
            ),
            (
                HISTORY_LIST_ITEMS_TOOL_NAME,
                &[
                    "limit",
                    "max_chars_per_item",
                    "recent_first",
                    "role",
                    "tool_name",
                    "tool_namespace",
                    "window_id",
                ],
                &[],
            ),
            (
                HISTORY_READ_ITEM_TOOL_NAME,
                &["item_id", "limit_chars", "offset_chars", "window_id"],
                &["item_id", "window_id"],
            ),
            (
                HISTORY_SEARCH_CONTENTS_TOOL_NAME,
                &[
                    "limit",
                    "query",
                    "recent_first",
                    "role",
                    "tool_name",
                    "tool_namespace",
                    "window_id",
                ],
                &["query"],
            ),
        ];
        let definitions = history_tool_definitions();
        for (name, properties, required) in expected {
            let definition = definitions
                .iter()
                .find(|definition| definition.name == name)
                .unwrap_or_else(|| panic!("缺少 history 工具定义：{name}"));
            assert_eq!(
                property_names(definition),
                name_set(properties),
                "{name} 的属性集必须与 Codex 对齐"
            );
            let required_fields = definition
                .schema
                .required_field_names()
                .into_iter()
                .collect::<BTreeSet<_>>();
            assert_eq!(
                required_fields,
                name_set(required),
                "{name} 的必填字段必须与 Codex 对齐"
            );
        }
    }

    #[test]
    fn history_tool_descriptions_are_product_ready_and_self_consistent() {
        for definition in history_tool_definitions() {
            let description = definition.description.as_str();
            assert!(
                !description.contains('{') && !description.contains('}'),
                "{} 的描述不得残留模板占位符",
                definition.name
            );
            assert!(
                !description.contains("最终一致"),
                "{} 的描述不得声明最终一致：本机归档同步扫描，立即可见",
                definition.name
            );
            assert!(
                description.contains("立即反映已归档内容"),
                "{} 的描述必须声明本地立即可见语义",
                definition.name
            );
            for property in property_names(&definition) {
                assert!(
                    definition.schema.input_schema["properties"][&property].is_object(),
                    "{} 的 {property} 必须带有字段级描述",
                    definition.name
                );
                let field_description =
                    definition.schema.input_schema["properties"][&property]["description"]
                        .as_str()
                        .unwrap_or_default();
                assert!(
                    !field_description.trim().is_empty(),
                    "{} 的 {property} 字段缺少 description",
                    definition.name
                );
            }
        }
    }

    #[test]
    fn history_optional_filters_match_codex_nullable_contract() {
        for definition in history_tool_definitions() {
            let properties = &definition.schema.input_schema["properties"];
            for field in [
                HISTORY_TOOL_NAMESPACE_FIELD,
                HISTORY_ROLE_FIELD,
                HISTORY_TOOL_NAME_FIELD,
                HISTORY_WINDOW_ID_FIELD,
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
    }
}
