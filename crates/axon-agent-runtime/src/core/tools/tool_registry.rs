use axon_crypto::sha256_hex;
use axon_domain::{
    AxonError, AxonResult, ToolDefinition, ToolInput, ToolInputFormat, ToolOutput,
    ToolOutputPolicy, ToolPermissionPolicy, ToolResult, ToolResultMetadata,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, RwLock},
};

use crate::{MCP_TOOL_NAMESPACE_PREFIX, mcp_provider_tool_name};

#[path = "tool_registry/tool_input_validation.rs"]
mod tool_input_validation;
pub use tool_input_validation::{ToolInputValidationError, ToolInputValidationIssue};

/// 工具注册表。
#[derive(Debug, Default, Clone)]
pub struct ToolRegistry {
    tools: Vec<ToolDefinition>,
    validators: HashMap<String, jsonschema::Validator>,
    qualified_index: HashMap<String, usize>,
    name_index: HashMap<String, usize>,
    qualified_name_restriction: Option<Vec<String>>,
    revision: u64,
    model_cache: Arc<RwLock<Option<ModelToolCache>>>,
}

#[derive(Debug, Clone)]
struct ModelToolCache {
    revision: u64,
    all: Vec<ModelToolDefinition>,
    provider: Vec<ModelToolDefinition>,
}

/// 模型可见工具定义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelToolDefinition {
    /// 完整 tool name。
    pub qualified_name: String,
    /// 稳定短描述。
    pub description: String,
    /// schema hash。
    pub schema_hash: String,
    /// 完整 Provider input schema。
    pub input_schema: serde_json::Value,
    /// Provider wire 输入格式。
    pub input_format: ToolInputFormat,
}

impl ToolRegistry {
    /// 创建空工具注册表。
    #[must_use]
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            validators: HashMap::new(),
            qualified_index: HashMap::new(),
            name_index: HashMap::new(),
            qualified_name_restriction: None,
            revision: 0,
            model_cache: Arc::new(RwLock::new(None)),
        }
    }

    /// 注册工具。
    pub fn register(&mut self, tool: ToolDefinition) -> AxonResult<()> {
        if !valid_identifier(&tool.namespace) || !valid_identifier(&tool.name) {
            return Err(AxonError::tool_invalid_definition(
                "tool namespace/name 必须符合 identifier 规则",
            ));
        }
        if reserved_namespace(&tool.namespace) {
            return Err(AxonError::tool_invalid_definition(
                "插件不能占用保留 namespace",
            ));
        }
        if tool.description.trim().is_empty() {
            return Err(AxonError::tool_invalid_definition(
                "tool description 不能为空",
            ));
        }
        let name_key = tool_name_key(&tool.namespace, &tool.name);
        let qualified_name = qualified_tool_name(&tool);
        if self.name_index.contains_key(&name_key)
            || self.qualified_index.contains_key(&qualified_name)
        {
            return Err(AxonError::tool_invalid_definition("tool 不能重复注册"));
        }
        let validator = jsonschema::validator_for(&tool.schema.input_schema).map_err(|error| {
            AxonError::tool_invalid_definition(format!(
                "tool input_schema 无效：{qualified_name}: {error}"
            ))
        })?;
        let index = self.tools.len();
        self.tools.push(tool);
        self.validators.insert(qualified_name.clone(), validator);
        self.name_index.insert(name_key, index);
        self.qualified_index.insert(qualified_name, index);
        self.revision = self.revision.wrapping_add(1);
        if let Ok(mut cache) = self.model_cache.write() {
            *cache = None;
        }
        Ok(())
    }

    /// 列出所有工具。
    #[must_use]
    pub fn tools(&self) -> &[ToolDefinition] {
        &self.tools
    }

    /// 列出只读工具。
    #[must_use]
    pub fn read_only_tools(&self) -> Vec<&ToolDefinition> {
        self.tools
            .iter()
            .filter(|tool| tool.permission_policy == ToolPermissionPolicy::ReadOnly)
            .collect()
    }

    /// 生成模型可见工具定义。
    #[must_use]
    pub fn model_tools(&self) -> Vec<ModelToolDefinition> {
        self.model_cache_snapshot().all
    }

    /// 生成 provider 可见工具定义；已发现的 MCP 工具直接使用自身 schema 调用。
    #[must_use]
    pub fn model_tools_for_provider(&self) -> Vec<ModelToolDefinition> {
        self.model_cache_snapshot().provider
    }

    /// 返回精确匹配 provider qualified name 的工具定义。
    #[must_use]
    pub fn tool_by_qualified_name(&self, qualified_name: &str) -> Option<&ToolDefinition> {
        self.qualified_index
            .get(qualified_name)
            .and_then(|index| self.tools.get(*index))
    }

    /// 执行前校验模型输出的工具输入。
    pub fn validate_input(&self, namespace: &str, name: &str, input: &ToolInput) -> AxonResult<()> {
        let tool = self.find(namespace, name)?;
        for field in tool.schema.required_field_names() {
            if !input.fields.iter().any(|candidate| candidate == &field) {
                return Err(AxonError::tool_invalid_definition(
                    "tool input 缺少必填字段",
                ));
            }
        }
        Ok(())
    }

    /// 将工具输出规范化为 ToolResult。
    pub fn materialize_output(
        &mut self,
        namespace: &str,
        name: &str,
        output: &ToolOutput,
        max_inline_bytes: Option<usize>,
    ) -> AxonResult<ToolResult> {
        self.materialize_output_with_archive_root(namespace, name, output, max_inline_bytes, None)
    }

    /// 将工具输出规范化并把超限正文写入调用方提供的持久化归档根。
    pub fn materialize_output_with_archive_root(
        &mut self,
        namespace: &str,
        name: &str,
        output: &ToolOutput,
        max_inline_bytes: Option<usize>,
        archive_root: Option<&Path>,
    ) -> AxonResult<ToolResult> {
        let tool = self.find(namespace, name)?.clone();
        let byte_len = output.text.len();
        let sha256 = sha256_hex(output.text.as_bytes());
        let truncated = tool.output_policy == ToolOutputPolicy::RefOnOverflow
            && max_inline_bytes.is_some_and(|limit| byte_len > limit);
        let summary = if truncated {
            utf8_prefix(&output.text, max_inline_bytes.unwrap_or_default()).to_owned()
        } else {
            output.text.clone()
        };
        let output_ref =
            if truncated {
                let archive_root = archive_root.ok_or_else(|| {
                    AxonError::tool_execution_failed("RefOnOverflow 缺少持久化 archive root")
                })?;
                let archived = crate::ContentArchiveRuntime::default()
                    .archive_text_with_preview(
                        archive_root,
                        crate::CONTENT_ARCHIVE_RESULT_STREAM,
                        &output.text,
                        &crate::tool_output_source_trace(namespace, name),
                        &summary,
                        true,
                    )
                    .map_err(|_| AxonError::tool_execution_failed("工具输出归档失败"))?;
                Some(archived.output_ref.ok_or_else(|| {
                    AxonError::tool_execution_failed("工具输出归档未生成 OutputRef")
                })?)
            } else {
                None
            };

        Ok(ToolResult {
            title: qualified_tool_name(&tool),
            summary: summary.clone(),
            inline_output: output_ref.is_none().then_some(summary),
            output_ref,
            metadata: ToolResultMetadata {
                truncated,
                byte_len,
                sha256,
            },
        })
    }

    fn find(&self, namespace: &str, name: &str) -> AxonResult<&ToolDefinition> {
        self.name_index
            .get(&tool_name_key(namespace, name))
            .and_then(|index| self.tools.get(*index))
            .ok_or(AxonError::tool_invalid_definition("tool 未注册"))
    }

    /// 按完整工具名构造受限注册表；空列表表示不限制。
    pub fn restricted_to_qualified_names(&self, names: &[String]) -> AxonResult<Self> {
        if names.is_empty() {
            return Ok(self.clone());
        }

        let mut restricted = Self::new();
        for name in names {
            let tool = self
                .qualified_index
                .get(name)
                .and_then(|index| self.tools.get(*index))
                .ok_or_else(|| {
                    AxonError::tool_invalid_definition(format!("tool 未注册：{name}"))
                })?;
            if restricted
                .tools
                .iter()
                .any(|candidate| qualified_tool_name(candidate) == *name)
            {
                continue;
            }
            restricted.register(tool.clone())?;
        }
        restricted.qualified_name_restriction =
            Some(restricted.tools.iter().map(qualified_tool_name).collect());
        Ok(restricted)
    }

    /// 将当前注册表的显式工具限制投影到刷新后的完整注册表。
    pub fn apply_restriction_to(&self, refreshed: &Self) -> AxonResult<Self> {
        match self.qualified_name_restriction.as_deref() {
            Some(names) => refreshed.restricted_to_qualified_names(names),
            None => Ok(refreshed.clone()),
        }
    }

    fn model_cache_snapshot(&self) -> ModelToolCache {
        if let Ok(cache) = self.model_cache.read()
            && let Some(cache) = cache
                .as_ref()
                .filter(|cache| cache.revision == self.revision)
        {
            return cache.clone();
        }
        let all = self
            .tools
            .iter()
            .filter(|tool| tool.enabled)
            .map(model_tool_definition)
            .collect::<Vec<_>>();
        let provider = all.clone();
        let snapshot = ModelToolCache {
            revision: self.revision,
            all,
            provider,
        };
        if let Ok(mut cache) = self.model_cache.write() {
            *cache = Some(snapshot.clone());
        }
        snapshot
    }
}

fn model_tool_definition(tool: &ToolDefinition) -> ModelToolDefinition {
    ModelToolDefinition {
        qualified_name: qualified_tool_name(tool),
        description: tool.description.clone(),
        schema_hash: schema_hash(tool),
        input_schema: tool.schema.input_schema.clone(),
        input_format: tool.schema.input_format.clone(),
    }
}

fn reserved_namespace(namespace: &str) -> bool {
    namespace == "axon.reserved" || namespace.starts_with("axon.reserved.")
}

fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '.')
}

fn qualified_tool_name(tool: &ToolDefinition) -> String {
    if let Some(server_id) = tool.namespace.strip_prefix(MCP_TOOL_NAMESPACE_PREFIX) {
        return mcp_provider_tool_name(server_id, &tool.name);
    }
    format!("{}.{}", tool.namespace, tool.name)
}

fn tool_name_key(namespace: &str, name: &str) -> String {
    format!("{namespace}\0{name}")
}

fn schema_hash(tool: &ToolDefinition) -> String {
    let input_schema =
        serde_json::to_string(&tool.schema.input_schema).unwrap_or_else(|_| "{}".to_owned());
    let input_format =
        serde_json::to_string(&tool.schema.input_format).unwrap_or_else(|_| "{}".to_owned());
    let payload = format!(
        "{}:{}:{}:{}:{}",
        tool.namespace, tool.name, tool.schema.version, input_schema, input_format
    );
    stable_hash_hex(payload.as_bytes())
}

fn stable_hash_hex(bytes: &[u8]) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut boundary = max_bytes.min(value.len());
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &value[..boundary]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axon_domain::{
        ToolId, ToolOutputPolicy, ToolPermissionPolicy, ToolSchema, ToolSchemaField,
    };

    #[test]
    fn restricted_registry_only_keeps_requested_tools_in_request_order() {
        let mut registry = ToolRegistry::new();
        registry
            .register(test_tool(ToolId(1), "builtin", "read_file"))
            .unwrap();
        registry
            .register(test_tool(ToolId(2), "builtin", "read_dir"))
            .unwrap();

        let restricted = registry
            .restricted_to_qualified_names(&[
                "builtin.read_dir".to_owned(),
                "builtin.read_file".to_owned(),
                "builtin.read_dir".to_owned(),
            ])
            .unwrap();

        assert_eq!(
            restricted
                .model_tools_for_provider()
                .iter()
                .map(|tool| tool.qualified_name.as_str())
                .collect::<Vec<_>>(),
            vec!["builtin.read_dir", "builtin.read_file"]
        );
        assert!(
            restricted
                .validate_input("builtin", "glob", &ToolInput { fields: Vec::new() },)
                .is_err()
        );
    }

    #[test]
    fn restricted_registry_rejects_unknown_tool() {
        let registry = ToolRegistry::new();
        let error = registry
            .restricted_to_qualified_names(&["builtin.missing".to_owned()])
            .unwrap_err();

        assert!(format!("{error:?}").contains("builtin.missing"));
    }

    #[test]
    fn restricted_registry_reapplies_allowlist_after_refresh() {
        let mut original = ToolRegistry::new();
        original
            .register(test_tool(ToolId(1), "builtin", "read_file"))
            .unwrap();
        original
            .register(test_tool(ToolId(2), "mcp__team", "query_db"))
            .unwrap();
        let restricted = original
            .restricted_to_qualified_names(&[
                "builtin.read_file".to_owned(),
                "mcp__team__query_db".to_owned(),
            ])
            .unwrap();

        let mut refreshed = original.clone();
        refreshed
            .register(test_tool(ToolId(3), "mcp__audit", "search_logs"))
            .unwrap();
        let reapplied = restricted.apply_restriction_to(&refreshed).unwrap();

        assert_eq!(
            reapplied
                .model_tools()
                .iter()
                .map(|tool| tool.qualified_name.as_str())
                .collect::<Vec<_>>(),
            vec!["builtin.read_file", "mcp__team__query_db"]
        );
    }

    #[test]
    fn schema_hash_changes_when_nested_schema_semantics_change() {
        let nested_schema = |mode_description: &str, modes: &[&str]| {
            ToolSchema::new(
                1,
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "options": {
                            "type": "object",
                            "properties": {
                                "mode": {
                                    "type": "string",
                                    "description": mode_description,
                                    "enum": modes
                                }
                            }
                        }
                    }
                }),
            )
        };
        let mut baseline = test_tool(ToolId(1), "mcp__demo", "query");
        baseline.schema = nested_schema("执行模式", &["fast", "safe"]);
        let mut description_changed = baseline.clone();
        description_changed.schema = nested_schema("查询执行模式", &["fast", "safe"]);
        let mut enum_changed = baseline.clone();
        enum_changed.schema = nested_schema("执行模式", &["fast", "safe", "audit"]);

        assert_ne!(schema_hash(&baseline), schema_hash(&description_changed));
        assert_ne!(schema_hash(&baseline), schema_hash(&enum_changed));
    }

    fn test_tool(id: ToolId, namespace: &str, name: &str) -> ToolDefinition {
        ToolDefinition {
            id,
            namespace: namespace.to_owned(),
            name: name.to_owned(),
            description: "Stable short description".to_owned(),
            schema: ToolSchema::object(
                1,
                ["query"],
                vec![ToolSchemaField::new(
                    "query",
                    axon_domain::ToolSchemaFieldKind::String,
                    "测试查询。",
                )],
            ),
            enabled: true,
            concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
            output_policy: ToolOutputPolicy::BoundedSummary,
            permission_policy: ToolPermissionPolicy::ReadOnly,
        }
    }
}
