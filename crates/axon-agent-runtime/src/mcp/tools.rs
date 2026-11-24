//! 插件 MCP tool schema 注册 runtime。

use std::collections::{BTreeMap, BTreeSet};

use axon_domain::{
    AxonError, AxonResult, McpServerDefinition, ToolConcurrencyPolicy, ToolDefinition, ToolId,
    ToolOutputPolicy, ToolPermissionPolicy, ToolSchema,
};
use serde_json::Value;

use crate::{
    MCP_TOOL_NAMESPACE_PREFIX, McpStartupProjection, ToolRegistry, mcp_provider_tool_name,
    mcp_tool_id, mcp_tool_namespace,
};

/// 插件 MCP tool cache anchor 前缀。
pub const MCP_TOOL_CACHE_ANCHOR_PREFIX: &str = "tool-schema://mcp";
/// MCP tool 默认 schema version。
pub const MCP_TOOL_SCHEMA_VERSION: u32 = 1;
const MCP_MODEL_TOOL_HASH_LEN: usize = 12;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const JSON_SCHEMA_TYPE_FIELD: &str = "type";
const JSON_SCHEMA_ARRAY_TYPE: &str = "array";
const JSON_SCHEMA_ITEMS_FIELD: &str = "items";

pub(crate) fn empty_mcp_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {},
        "required": [],
        "additionalProperties": false,
    })
}

/// MCP tool 安全注解。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpToolAnnotations {
    /// server 声明该工具为只读。
    pub read_only_hint: Option<bool>,
    /// server 声明该工具可能执行破坏性操作。
    pub destructive_hint: Option<bool>,
    /// server 声明该工具可能访问开放世界资源。
    pub open_world_hint: Option<bool>,
}

impl McpToolAnnotations {
    /// MCP 工具的权限策略：已发现的工具直接可调用，不进入人工审批。
    ///
    /// 权限不再依赖本地信任开关：只有 server 完整声明三项安全注解时才记为
    /// `ReadOnly`，其余一律 `GovernedWrite`。两者都不需要人工审批，因此模型
    /// 不会因为审批摩擦而回避已配置的 MCP 能力。
    #[must_use]
    pub fn permission_policy(self) -> ToolPermissionPolicy {
        if self.is_explicitly_safe() {
            ToolPermissionPolicy::ReadOnly
        } else {
            ToolPermissionPolicy::GovernedWrite
        }
    }

    /// 根据 MCP 安全注解推导并发策略。
    #[must_use]
    pub fn concurrency_policy(self) -> ToolConcurrencyPolicy {
        if self.is_explicitly_safe() {
            ToolConcurrencyPolicy::ParallelSafe
        } else {
            ToolConcurrencyPolicy::Exclusive
        }
    }

    const fn is_explicitly_safe(self) -> bool {
        matches!(self.read_only_hint, Some(true))
            && matches!(self.destructive_hint, Some(false))
            && matches!(self.open_world_hint, Some(false))
    }
}

/// 插件 MCP tool schema 声明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolSchema {
    /// 模型可见 tool 名称。
    pub name: String,
    /// MCP server 暴露的原始 tool 名称，执行 `tools/call` 时原样回传。
    pub mcp_name: String,
    /// 稳定短描述。
    pub description: String,
    /// MCP server 暴露的完整 inputSchema。
    pub input_schema: Value,
    /// MCP server 暴露的完整 outputSchema。
    pub output_schema: Option<Value>,
    /// MCP tool 安全注解。
    pub annotations: McpToolAnnotations,
}

impl McpToolSchema {
    /// 从已发现的 MCP tool 摘要构造 schema，并保留其安全注解。
    #[must_use]
    pub fn from_discovered_tool(tool: &crate::McpDiscoveredTool) -> Self {
        Self {
            name: tool.name.clone(),
            mcp_name: tool.name.clone(),
            description: tool.description.clone(),
            input_schema: tool.input_schema.clone(),
            output_schema: tool.output_schema.clone(),
            annotations: tool.annotations,
        }
    }
}

/// 插件 MCP tool 注册报告。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpToolRegistrationReport {
    /// 注册到 `ToolRegistry` 的工具定义。
    pub registered_tools: Vec<ToolDefinition>,
    /// 与注册工具一一对应的规范化 schema。
    pub registered_schemas: Vec<McpToolSchema>,
    /// schema hash 参与的 cache anchor。
    pub cache_anchor: String,
    /// 是否启动真实 MCP server。
    pub server_process_started: bool,
}

/// 插件 MCP tool schema 注册 runtime。
#[derive(Debug, Default, Clone, Copy)]
pub struct McpToolRegistryRuntime;

impl McpToolRegistryRuntime {
    /// 将 MCP tool schema 注册进 `ToolRegistry` 并生成 cache anchor；不执行工具。
    pub fn register_tool_schemas(
        registry: &mut ToolRegistry,
        report: &McpServerDefinition,
        startup: &McpStartupProjection,
        server_id: &str,
        tool_schemas: &[McpToolSchema],
        first_tool_id: ToolId,
    ) -> AxonResult<McpToolRegistrationReport> {
        ensure_mcp_capability(report, startup)?;
        let registered_schemas = normalize_mcp_tool_schemas_for_model(server_id, tool_schemas)?;
        let tools = mcp_tools(server_id, &registered_schemas, first_tool_id);
        let mut candidate = registry.clone();
        for tool in &tools {
            candidate.register(tool.clone())?;
        }
        let cache_anchor = mcp_cache_anchor(&candidate, &tools);
        *registry = candidate;

        Ok(McpToolRegistrationReport {
            registered_tools: tools,
            registered_schemas,
            cache_anchor,
            server_process_started: false,
        })
    }
}

fn ensure_mcp_capability(
    report: &McpServerDefinition,
    startup: &McpStartupProjection,
) -> AxonResult<()> {
    debug_assert!(!report.server_id.is_empty());
    if startup.server_process_started {
        return Err(AxonError::InvalidMcpDefinition(
            "MCP tool schema 注册阶段不能要求真实 server 已启动",
        ));
    }
    Ok(())
}

fn mcp_tools(
    server_id: &str,
    tool_schemas: &[McpToolSchema],
    first_tool_id: ToolId,
) -> Vec<ToolDefinition> {
    let namespace = mcp_tool_namespace(server_id);
    let mut tools = Vec::new();

    for schema in tool_schemas {
        tools.push(ToolDefinition {
            id: ToolId(first_tool_id.0 + tools.len() as u64),
            namespace: namespace.clone(),
            name: schema.name.clone(),
            description: schema.description.clone(),
            schema: ToolSchema::new(MCP_TOOL_SCHEMA_VERSION, schema.input_schema.clone()),
            enabled: true,
            concurrency_policy: schema.annotations.concurrency_policy(),
            output_policy: ToolOutputPolicy::RefOnOverflow,
            permission_policy: schema.annotations.permission_policy(),
        });
    }

    tools
}

/// 将 MCP 原始 tool 名称投影为模型可见、稳定去重的本地工具名。
pub fn normalize_mcp_tool_schemas_for_model(
    server_id: &str,
    tool_schemas: &[McpToolSchema],
) -> AxonResult<Vec<McpToolSchema>> {
    let mut base_names = BTreeMap::<String, BTreeSet<String>>::new();
    let mut candidates = Vec::new();
    for schema in tool_schemas {
        let mcp_name = original_mcp_tool_name(schema)?;
        let base_name = mcp_tool_id(&schema.name)?;
        base_names
            .entry(base_name.clone())
            .or_default()
            .insert(mcp_name.clone());
        candidates.push((base_name, schema, mcp_name));
    }

    let colliding_bases = base_names
        .into_iter()
        .filter_map(|(base_name, names)| (names.len() > 1).then_some(base_name))
        .collect::<BTreeSet<_>>();
    let mut used_provider_names = BTreeSet::new();
    let mut normalized = Vec::new();

    for (base_name, schema, mcp_name) in candidates {
        let name = unique_model_name(
            server_id,
            &base_name,
            &mcp_name,
            colliding_bases.contains(&base_name),
            &mut used_provider_names,
        );
        normalized.push(McpToolSchema {
            name,
            mcp_name,
            description: schema.description.clone(),
            input_schema: normalize_mcp_input_schema(&schema.input_schema),
            output_schema: schema.output_schema.clone(),
            annotations: schema.annotations,
        });
    }

    Ok(normalized)
}

fn normalize_mcp_input_schema(schema: &Value) -> Value {
    let mut normalized = schema.clone();
    ensure_array_items(&mut normalized);
    normalized
}

fn ensure_array_items(schema: &mut Value) {
    match schema {
        Value::Object(fields) => {
            if fields.get(JSON_SCHEMA_TYPE_FIELD).and_then(Value::as_str)
                == Some(JSON_SCHEMA_ARRAY_TYPE)
            {
                fields
                    .entry(JSON_SCHEMA_ITEMS_FIELD)
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
            }
            for value in fields.values_mut() {
                ensure_array_items(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                ensure_array_items(value);
            }
        }
        _ => {}
    }
}

fn original_mcp_tool_name(schema: &McpToolSchema) -> AxonResult<String> {
    let mcp_name = if schema.mcp_name.trim().is_empty() {
        schema.name.trim()
    } else {
        schema.mcp_name.trim()
    };
    if mcp_name.is_empty() {
        return Err(AxonError::InvalidMcpDefinition("MCP tool 名称不能为空"));
    }
    Ok(mcp_name.to_owned())
}

fn unique_model_name(
    server_id: &str,
    base_name: &str,
    mcp_name: &str,
    force_hash: bool,
    used_provider_names: &mut BTreeSet<String>,
) -> String {
    let mut attempt = 0_u32;
    loop {
        let identity = if attempt == 0 {
            format!("{server_id}\0{mcp_name}")
        } else {
            format!("{server_id}\0{mcp_name}\0{attempt}")
        };
        let name = if force_hash || attempt > 0 {
            format!("{base_name}{}", stable_hash_suffix(&identity))
        } else {
            base_name.to_owned()
        };
        if used_provider_names.insert(mcp_provider_tool_name(server_id, &name)) {
            return name;
        }
        attempt = attempt.saturating_add(1);
    }
}

fn stable_hash_suffix(value: &str) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("_{hash:016x}")
        .chars()
        .take(MCP_MODEL_TOOL_HASH_LEN + 1)
        .collect()
}

fn mcp_cache_anchor(registry: &ToolRegistry, tools: &[ToolDefinition]) -> String {
    let registered_names = tools
        .iter()
        .filter_map(|tool| {
            let server_id = tool.namespace.strip_prefix(MCP_TOOL_NAMESPACE_PREFIX)?;
            Some(mcp_provider_tool_name(server_id, &tool.name))
        })
        .collect::<BTreeSet<_>>();
    let schema_hashes = registry
        .model_tools()
        .into_iter()
        .filter(|tool| registered_names.contains(&tool.qualified_name))
        .map(|tool| tool.schema_hash)
        .collect::<Vec<_>>()
        .join(",");
    format!("{MCP_TOOL_CACHE_ANCHOR_PREFIX}:{schema_hashes}")
}

#[cfg(test)]
mod tests {
    use super::*;

    use axon_domain::McpServerStartupStatus;

    #[test]
    fn mcp_tool_schema_registers_with_cache_anchor() {
        let mut registry = ToolRegistry::new();
        let startup = McpStartupProjection {
            server_id: "team-tools".to_owned(),
            config_ref: "mcp://team-tools/.mcp.json".to_owned(),
            status: McpServerStartupStatus::Discovered,
            server_process_started: false,
            remote_trigger_allowed: false,
            audit_summary: "MCP startup 已发现".to_owned(),
            reasons: vec![],
        };

        let report = McpToolRegistryRuntime::register_tool_schemas(
            &mut registry,
            &mcp_report(),
            &startup,
            "team_tools",
            &[McpToolSchema {
                name: "query-db".to_owned(),
                mcp_name: "query-db".to_owned(),
                description: "查询数据库 schema".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "数据库查询" }
                    },
                    "required": ["query"]
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "rows": { "type": "array" } },
                    "required": ["rows"]
                })),
                annotations: McpToolAnnotations {
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    open_world_hint: Some(false),
                },
            }],
            ToolId(900),
        )
        .unwrap();

        assert_eq!(report.registered_tools.len(), 1);
        assert!(
            report
                .cache_anchor
                .starts_with(MCP_TOOL_CACHE_ANCHOR_PREFIX)
        );
        assert!(!report.server_process_started);
        assert_eq!(
            report.registered_tools[0].permission_policy,
            ToolPermissionPolicy::GovernedWrite,
            "已发现的 MCP 工具必须无需人工审批即可直接调用"
        );
        assert_eq!(
            report.registered_tools[0].concurrency_policy,
            ToolConcurrencyPolicy::Exclusive
        );
        assert!(registry.model_tools().iter().any(|tool| {
            tool.qualified_name == "mcp__team_tools__query_db" && !tool.schema_hash.is_empty()
        }));
        assert!(
            registry
                .model_tools_for_provider()
                .iter()
                .any(|tool| tool.qualified_name == "mcp__team_tools__query_db")
        );
    }

    #[test]
    fn mcp_policy_only_annotations_affect_scheduling() {
        let safe = McpToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            open_world_hint: Some(false),
        };
        let risky_open_world = McpToolAnnotations {
            open_world_hint: Some(true),
            ..safe
        };
        let incomplete = McpToolAnnotations {
            destructive_hint: None,
            ..safe
        };

        assert_eq!(
            safe.concurrency_policy(),
            ToolConcurrencyPolicy::ParallelSafe
        );
        assert_eq!(safe.permission_policy(), ToolPermissionPolicy::ReadOnly);
        // 缺少完整安全注解只影响并发调度，绝不把工具推回人工审批。
        for annotations in [McpToolAnnotations::default(), risky_open_world, incomplete] {
            assert_eq!(
                annotations.permission_policy(),
                ToolPermissionPolicy::GovernedWrite
            );
            assert_eq!(
                annotations.concurrency_policy(),
                ToolConcurrencyPolicy::Exclusive
            );
        }
    }

    #[test]
    fn mcp_tool_schema_adds_open_items_to_unconstrained_arrays() {
        let schemas = normalize_mcp_tool_schemas_for_model(
            "epaasctl",
            &[McpToolSchema {
                name: "epaas_db_biz_query".to_owned(),
                mcp_name: "epaas_db_biz_query".to_owned(),
                description: "查询业务对象".to_owned(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "orders": { "type": "array" },
                        "filters": {
                            "anyOf": [
                                { "type": "array" },
                                { "type": "null" }
                            ]
                        },
                        "fields": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }),
                output_schema: None,
                annotations: McpToolAnnotations::default(),
            }],
        )
        .unwrap();
        let properties = &schemas[0].input_schema["properties"];

        assert_eq!(properties["orders"]["items"], serde_json::json!({}));
        assert_eq!(
            properties["filters"]["anyOf"][0]["items"],
            serde_json::json!({})
        );
        assert_eq!(
            properties["fields"]["items"],
            serde_json::json!({ "type": "string" })
        );
    }

    #[test]
    fn mcp_tool_collision_names_are_provider_safe_and_schema_stable() {
        let first = colliding_schemas("查询数据库", vec!["query".to_owned()]);
        let changed_schema = colliding_schemas(
            "查询数据库与审计信息",
            vec!["query".to_owned(), "limit".to_owned()],
        );

        let first = normalize_mcp_tool_schemas_for_model("team_tools", &first).unwrap();
        let changed = normalize_mcp_tool_schemas_for_model("team_tools", &changed_schema).unwrap();
        let names = first
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<BTreeSet<_>>();
        let provider_names = first
            .iter()
            .map(|schema| mcp_provider_tool_name("team_tools", &schema.name))
            .collect::<Vec<_>>();

        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|name| name.starts_with("query_db_")));
        assert!(
            provider_names
                .iter()
                .all(|name| name.len() <= crate::MCP_PROVIDER_TOOL_NAME_MAX_LEN)
        );
        assert_eq!(
            first.iter().map(|schema| &schema.name).collect::<Vec<_>>(),
            changed
                .iter()
                .map(|schema| &schema.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            first
                .iter()
                .map(|schema| schema.mcp_name.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["query-db", "query_db"])
        );
    }

    fn colliding_schemas(
        first_description: &str,
        first_required_fields: Vec<String>,
    ) -> Vec<McpToolSchema> {
        let input_schema = |required: Vec<String>| {
            serde_json::json!({
                "type": "object",
                "properties": required
                    .iter()
                    .map(|field| (field.clone(), serde_json::json!({ "type": "string" })))
                    .collect::<serde_json::Map<_, _>>(),
                "required": required,
            })
        };
        vec![
            McpToolSchema {
                name: "query-db".to_owned(),
                mcp_name: "query-db".to_owned(),
                description: first_description.to_owned(),
                input_schema: input_schema(first_required_fields),
                output_schema: None,
                annotations: McpToolAnnotations::default(),
            },
            McpToolSchema {
                name: "query_db".to_owned(),
                mcp_name: "query_db".to_owned(),
                description: "查询仓库".to_owned(),
                input_schema: input_schema(vec!["query".to_owned()]),
                output_schema: None,
                annotations: McpToolAnnotations::default(),
            },
        ]
    }

    fn mcp_report() -> McpServerDefinition {
        McpServerDefinition::new("team-tools").unwrap()
    }
}
