//! `mcp_schema_support` 的可观察行为测试：熔断期工具可见性与降级标记。

use std::{collections::BTreeMap, path::Path};

use axon_agent_runtime::{McpServerCommand, McpToolAnnotations};
use axon_domain::{McpServerDefinition, McpServerStartupStatus};

use super::*;
use crate::discovery::{McpServerSource, RuntimeMcpServer};
use crate::mcp_health_cache::record_mcp_health_failure;
use crate::mcp_schema_cache::{McpSchemaCacheEntry, mcp_config_fingerprint, store_mcp_schemas};

fn schema(name: &str) -> McpToolSchema {
    McpToolSchema {
        name: name.to_owned(),
        mcp_name: name.to_owned(),
        description: "测试工具".to_owned(),
        input_schema: serde_json::json!({"type": "object", "properties": {}}),
        output_schema: None,
        annotations: McpToolAnnotations {
            read_only_hint: Some(true),
            destructive_hint: Some(false),
            open_world_hint: Some(false),
        },
    }
}

fn stdio_config() -> McpServerConfig {
    // 熔断命中时绝不会拉起进程；这个 program 不能存在，正好证明「没有真的探测」。
    McpServerConfig::Stdio(McpServerCommand {
        program: "axon-test-mcp-server-not-installed".to_owned(),
        args: Vec::new(),
        working_dir: None,
        env: BTreeMap::new(),
        tool_timeout_ms: None,
    })
}

fn install(root: &Path) -> RuntimeMcpServer {
    RuntimeMcpServer {
        definition: McpServerDefinition::new("mcp_demo").expect("server ID 合法"),
        source: McpServerSource::User,
        config_path: root.join(".mcp.json"),
        server_name: "demo".to_owned(),
        config: stdio_config(),
    }
}

fn startup(install: &RuntimeMcpServer) -> McpStartupProjection {
    McpStartupProjection {
        server_id: install.server_id().to_owned(),
        config_ref: install.config_ref(),
        status: McpServerStartupStatus::Discovered,
        server_process_started: false,
        remote_trigger_allowed: false,
        audit_summary: String::new(),
        reasons: Vec::new(),
    }
}

fn test_root(tag: &str) -> std::path::PathBuf {
    axon_test_support::test_temp_root().join(format!(
        "axon-mcp-schema-support-{tag}-{}-{}",
        std::process::id(),
        axon_time::unix_now_ms()
    ))
}

/// 发现期熔断命中负缓存时，工具不能整批消失：沿用上次目录并标出「暂时不可用」。
#[test]
fn negative_health_cache_keeps_last_known_tools_visible_with_marker() {
    let root = test_root("degraded");
    std::fs::create_dir_all(&root).unwrap();
    let install = install(&root);
    let startup = startup(&install);
    store_mcp_schemas(
        &root,
        &McpSchemaCacheEntry {
            server_id: "mcp_demo",
            config_ref: &startup.config_ref,
            config_fingerprint: &mcp_config_fingerprint(&install.config),
            metadata_fingerprint: "meta-v1",
            tools_list_changed: None,
            refreshed_at_ms: 1,
        },
        &[schema("read")],
    )
    .expect("缓存目录应可写入");
    record_mcp_health_failure(
        &root,
        &startup.config_ref,
        "MCP HTTP tools/list 失败：握手超时",
    );

    let load = load_runtime_mcp_tool_schemas(
        &root,
        &builtin_tool_registry(),
        &install,
        &startup,
        "mcp_demo",
        false,
    );

    assert_eq!(load.disposition.as_str(), "degraded");
    let (_, schemas) = load.loaded.expect("熔断期必须沿用上次工具目录");
    assert_eq!(schemas.len(), 1);
    assert!(
        schemas[0].description.contains(MCP_DEGRADED_TOOL_MARKER),
        "工具描述必须带可见的不可用标记：{}",
        schemas[0].description
    );
    assert!(
        schemas[0].description.contains("server=mcp_demo"),
        "标记必须指明是哪台 server：{}",
        schemas[0].description
    );
    assert!(load.failure.expect("失败原因仍要留痕").contains("握手超时"));
    let _ = std::fs::remove_dir_all(root);
}

/// 没有上次成功目录时无从沿革：保持原有行为（不注册工具），仍如实带失败原因。
#[test]
fn negative_health_cache_without_cached_catalog_stays_empty() {
    let root = test_root("degraded-empty");
    std::fs::create_dir_all(&root).unwrap();
    let install = install(&root);
    let startup = startup(&install);
    record_mcp_health_failure(
        &root,
        &startup.config_ref,
        "MCP tools/list 失败：连接被拒绝",
    );

    let load = load_runtime_mcp_tool_schemas(
        &root,
        &builtin_tool_registry(),
        &install,
        &startup,
        "mcp_demo",
        false,
    );

    assert!(load.loaded.is_none(), "没有缓存目录时不能凭空注册工具");
    assert_eq!(load.disposition.as_str(), "miss");
    assert!(
        load.failure
            .expect("失败原因必须带出")
            .contains("连接被拒绝")
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 标记必须可重复叠加而不再增长：同一状态两次构建要产出同一段描述。
#[test]
fn degraded_marker_is_idempotent() {
    let once = mark_degraded_schemas("mcp_demo", vec![schema("read")]);
    let twice = mark_degraded_schemas("mcp_demo", once.clone());

    assert_eq!(once, twice, "重复加载不能把标记叠加成多份");
    assert_eq!(once[0].name, "read");
    assert_eq!(once[0].mcp_name, "read");
}
