//! 工具领域对象。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{OutputRefId, ToolId};

/// 工具定义。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDefinition {
    /// 工具 ID。
    pub id: ToolId,
    /// 工具命名空间。
    pub namespace: String,
    /// 工具名称。
    pub name: String,
    /// 稳定短描述。
    pub description: String,
    /// 输入 schema。
    pub schema: ToolSchema,
    /// 是否启用。
    pub enabled: bool,
    /// 并发执行策略。
    pub concurrency_policy: ToolConcurrencyPolicy,
    /// 输出策略。
    pub output_policy: ToolOutputPolicy,
    /// 权限策略。
    pub permission_policy: ToolPermissionPolicy,
}

/// 工具并发执行策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolConcurrencyPolicy {
    /// 工具不修改共享状态，可与同批其他工具并行执行。
    ParallelSafe,
    /// 工具可能修改工作区，由运行时按路径或工作区范围协调写入。
    WorkspaceCoordinated,
    /// 工具涉及无法安全协调的共享状态，必须独占当前工具批次。
    Exclusive,
}

/// 工具输入 schema。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    /// 稳定 schema 版本。
    pub version: u32,
    /// 完整 JSON Schema；Provider、MCP 缓存、能力检索与执行校验共同使用。
    pub input_schema: Value,
    /// Provider wire 输入格式。
    pub input_format: ToolInputFormat,
}

impl ToolSchema {
    /// 直接使用完整 JSON Schema 创建工具 schema。
    #[must_use]
    pub fn new(version: u32, input_schema: Value) -> Self {
        Self {
            version,
            input_schema,
            input_format: ToolInputFormat::JsonSchema,
        }
    }

    /// 创建 freeform 工具 schema；input_schema 用于非 custom Provider 的可逆 function fallback。
    #[must_use]
    pub fn freeform(
        version: u32,
        input_schema: Value,
        fallback_field: impl Into<String>,
        format: ToolFreeformFormat,
    ) -> Self {
        Self {
            version,
            input_schema,
            input_format: ToolInputFormat::Freeform {
                fallback_field: fallback_field.into(),
                format,
            },
        }
    }

    /// 从字段声明一次性构造完整 object JSON Schema。
    #[must_use]
    pub fn object<I, S>(version: u32, required_fields: I, fields: Vec<ToolSchemaField>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let properties = fields
            .into_iter()
            .map(|field| (field.name.clone(), field.json_schema()))
            .collect::<Map<_, _>>();
        Self {
            version,
            input_schema: json!({
                "type": "object",
                "properties": properties,
                "required": required_fields.into_iter().map(Into::into).collect::<Vec<String>>(),
                "additionalProperties": false,
            }),
            input_format: ToolInputFormat::JsonSchema,
        }
    }

    /// 返回顶层必填字段名。
    #[must_use]
    pub fn required_field_names(&self) -> Vec<String> {
        self.input_schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect()
    }

    /// 返回用于 capability routing 的顶层字段名。
    #[must_use]
    pub fn field_names(&self) -> Vec<String> {
        self.input_schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|properties| properties.keys().cloned())
            .collect()
    }
}

/// 工具 schema 字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchemaField {
    /// 字段名。
    pub name: String,
    /// 字段类型。
    pub kind: ToolSchemaFieldKind,
    /// 字段用途、约束与默认行为。
    pub description: String,
}

impl ToolSchemaField {
    /// 创建带语义说明的工具字段。
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        kind: ToolSchemaFieldKind,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            description: description.into(),
        }
    }

    fn json_schema(&self) -> Value {
        let mut schema = match self.kind {
            ToolSchemaFieldKind::String => json!({ "type": "string" }),
            ToolSchemaFieldKind::NonBlankString => json!({
                "type": "string",
                "pattern": r"\S",
            }),
            ToolSchemaFieldKind::StringEnum(values) => json!({
                "type": "string",
                "enum": values,
            }),
            ToolSchemaFieldKind::StringArray => json!({
                "type": "array",
                "items": { "type": "string" },
            }),
            ToolSchemaFieldKind::NonBlankStringArray => json!({
                "type": "array",
                "items": {
                    "type": "string",
                    "pattern": r"\S",
                },
            }),
            ToolSchemaFieldKind::IntegerArray => json!({
                "type": "array",
                "items": { "type": "integer", "minimum": 1 },
                "uniqueItems": true,
            }),
            ToolSchemaFieldKind::Boolean => json!({ "type": "boolean" }),
            ToolSchemaFieldKind::Integer => json!({
                "type": "integer",
                "minimum": 0,
            }),
        };
        if !self.description.trim().is_empty() {
            schema["description"] = Value::String(self.description.clone());
        }
        schema
    }
}

/// 工具 schema 字段类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSchemaFieldKind {
    /// 可为空字符串。
    String,
    /// 至少包含一个非空白字符的字符串。
    NonBlankString,
    /// 有限字符串枚举。
    StringEnum(&'static [&'static str]),
    /// 元素可为空的字符串数组。
    StringArray,
    /// 元素至少包含一个非空白字符的字符串数组。
    NonBlankStringArray,
    /// 正整数数组。
    IntegerArray,
    /// 布尔值。
    Boolean,
    /// 非负整数。
    Integer,
}

/// 工具输出策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOutputPolicy {
    /// 只返回 bounded summary。
    BoundedSummary,
    /// 超限时转 `OutputRef`。
    RefOnOverflow,
}

/// 工具权限策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPermissionPolicy {
    /// 只读。
    ReadOnly,
    /// 由运行时准入门禁治理的写入。
    ///
    /// 覆盖内部写入工具，以及注册后即可直接调用的外部工具（如 MCP 工具）。
    GovernedWrite,
    /// 权限取决于本次调用参数。
    CallEvaluated,
}

/// 单次工具调用的执行权限。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolExecutionPermission {
    /// 本次调用允许直接执行。
    Allowed,
    /// 本次调用被明确禁止执行。
    Forbidden {
        /// 禁止执行的稳定原因。
        reason: String,
    },
}

/// 工具调用输入。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInput {
    /// 字段名列表。
    pub fields: Vec<String>,
}

/// 工具输出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    /// 原始输出文本。
    pub text: String,
}

/// 工具结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// 标题。
    pub title: String,
    /// bounded summary。
    pub summary: String,
    /// 内联输出。
    pub inline_output: Option<String>,
    /// 输出引用。
    pub output_ref: Option<OutputRef>,
    /// metadata。
    pub metadata: ToolResultMetadata,
}

/// 工具结果 metadata。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResultMetadata {
    /// 是否截断。
    pub truncated: bool,
    /// 原始字节数。
    pub byte_len: usize,
    /// 输出 hash。
    pub sha256: String,
}

/// 工具输出引用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputRef {
    /// OutputRef ID。
    pub id: OutputRefId,
    /// 原始字节数。
    pub byte_len: usize,
    /// 内容 hash。
    pub sha256: String,
    /// 来源追踪。
    pub source_trace: String,
    /// bounded preview。
    pub preview: String,
    /// 可由 archive runtime 读取的相对引用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_ref: Option<OutputArchiveReference>,
}

/// 工具输出归档引用；路径相对于调用方提供的 archive root。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputArchiveReference {
    /// 归档存储内的相对路径。
    pub relative_path: String,
}

/// 工具输入的 Provider wire 格式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolInputFormat {
    /// 通过 JSON Schema 约束的 function 输入。
    JsonSchema,
    /// OpenAI Responses custom tool 使用的 freeform 输入。
    Freeform {
        /// 非 custom Provider 使用 function call 时承载原文的字段。
        fallback_field: String,
        /// Provider 原生 freeform 格式。
        format: ToolFreeformFormat,
    },
}

/// freeform tool 的 Provider 原生格式。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolFreeformFormat {
    /// 格式类型，例如 grammar。
    pub format_type: String,
    /// grammar 语法，例如 lark。
    pub syntax: String,
    /// 完整 grammar 定义。
    pub definition: String,
}

/// Provider 与历史共同使用的工具调用输入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "camelCase")]
pub enum ToolCallInput {
    /// 已解析的 JSON function 输入。
    Json(Value),
    /// custom/freeform 原始文本输入。
    Text(String),
}

impl ToolCallInput {
    /// 创建 JSON 工具输入。
    #[must_use]
    pub fn json(value: Value) -> Self {
        Self::Json(value)
    }

    /// 创建 freeform 文本工具输入。
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// 返回 JSON 输入。
    #[must_use]
    pub const fn as_json(&self) -> Option<&Value> {
        match self {
            Self::Json(value) => Some(value),
            Self::Text(_) => None,
        }
    }

    /// 返回 freeform 文本输入。
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Json(_) => None,
            Self::Text(value) => Some(value),
        }
    }

    /// 返回用于日志、缓存和本机解析的稳定文本。
    #[must_use]
    pub fn canonical_text(&self) -> String {
        match self {
            Self::Json(value) => serde_json::to_string(value).unwrap_or_else(|_| "null".to_owned()),
            Self::Text(value) => value.clone(),
        }
    }

    /// 编码为 function call JSON；freeform 输入使用 schema 声明的 fallback 字段。
    pub fn to_function_arguments(&self, fallback_field: Option<&str>) -> Result<Value, String> {
        match self {
            Self::Json(value) => Ok(value.clone()),
            Self::Text(value) => {
                let field = fallback_field
                    .filter(|field| !field.trim().is_empty())
                    .ok_or_else(|| "freeform 工具缺少 function fallback 字段".to_owned())?;
                Ok(json!({ field: value }))
            }
        }
    }
}

impl From<Value> for ToolCallInput {
    fn from(value: Value) -> Self {
        Self::Json(value)
    }
}

impl From<String> for ToolCallInput {
    fn from(value: String) -> Self {
        serde_json::from_str(&value).map_or(Self::Text(value), Self::Json)
    }
}

impl From<&str> for ToolCallInput {
    fn from(value: &str) -> Self {
        value.to_owned().into()
    }
}
