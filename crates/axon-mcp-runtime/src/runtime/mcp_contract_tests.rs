use axon_agent_runtime::ToolRegistry;
use axon_domain::{
    ToolConcurrencyPolicy, ToolDefinition, ToolId, ToolOutputPolicy, ToolPermissionPolicy,
    ToolSchema,
};

use super::ensure_runtime_mcp_contract_unchanged;

const TEST_TOOL_NAME: &str = "mcp__team__query_db";

#[test]
fn execution_rejects_schema_or_policy_drift_from_turn_snapshot() {
    let expected = registry_with_contract(serde_json::json!(["query"]), true);
    let same = registry_with_contract(serde_json::json!(["query"]), true);
    let changed_schema = registry_with_contract(serde_json::json!(["query", "limit"]), true);
    let changed_policy = registry_with_contract(serde_json::json!(["query"]), false);

    assert!(ensure_runtime_mcp_contract_unchanged(&expected, &same, TEST_TOOL_NAME).is_ok());
    assert!(
        ensure_runtime_mcp_contract_unchanged(&expected, &changed_schema, TEST_TOOL_NAME).is_err()
    );
    assert!(
        ensure_runtime_mcp_contract_unchanged(&expected, &changed_policy, TEST_TOOL_NAME).is_err()
    );
}

fn registry_with_contract(required: serde_json::Value, read_only: bool) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry
        .register(ToolDefinition {
            id: ToolId(900),
            namespace: "mcp__team".to_owned(),
            name: "query_db".to_owned(),
            description: "查询数据库".to_owned(),
            schema: ToolSchema::new(
                1,
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer"}
                    },
                    "required": required,
                    "additionalProperties": false
                }),
            ),
            enabled: true,
            concurrency_policy: if read_only {
                ToolConcurrencyPolicy::ParallelSafe
            } else {
                ToolConcurrencyPolicy::Exclusive
            },
            output_policy: ToolOutputPolicy::RefOnOverflow,
            permission_policy: if read_only {
                ToolPermissionPolicy::ReadOnly
            } else {
                ToolPermissionPolicy::GovernedWrite
            },
        })
        .expect("测试 MCP 工具应可注册");
    registry
}
