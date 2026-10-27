use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axon_domain::{CellId, ModelProfileId, PermissionSnapshotId, SessionId};
use axon_mcp_runtime::RuntimeMcpToolCallFailureKind;

fn required_fields(tool: &axon_agent_runtime::ModelToolDefinition) -> Vec<&str> {
    tool.input_schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect()
}

#[test]
fn runtime_tool_registry_exposes_mcp_tool_for_turn_time_use() {
    let root = super::test_root("runtime-tools");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_handshake_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");

    let registry =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;
    let model_tools = registry.model_tools();

    assert!(model_tools.iter().any(|tool| {
        tool.qualified_name == "builtin.list_mcp_resources" && required_fields(tool).is_empty()
    }));
    assert!(model_tools.iter().any(|tool| {
        tool.qualified_name == "builtin.read_mcp_resource"
            && required_fields(tool) == ["server", "uri"]
    }));
    for removed_tool in [
        "builtin.list_mcp_resource_templates",
        "builtin.list_mcp_prompts",
        "builtin.get_mcp_prompt",
    ] {
        assert!(
            model_tools
                .iter()
                .all(|tool| tool.qualified_name != removed_tool),
            "{removed_tool} 不应暴露给模型"
        );
    }
    assert!(model_tools.iter().any(|tool| {
        tool.qualified_name == "mcp__team_tools__query_db" && required_fields(tool) == ["query"]
    }));

    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_builtin_mcp_resource_tools_read_server_resources() {
    let root = super::test_root("runtime-mcp-resources");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_runtime_exec_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");

    let list = crate::runtime_tools::execute_runtime_mcp_list_resources(
        &config_path,
        None,
        Some("team-tools"),
        None,
    )
    .expect("模型侧 list_mcp_resources 应可读取 MCP resources");
    assert!(list.summary.contains("axon://team-tools/schema"));
    assert_eq!(list.metadata["rawPayloadExposed"], false);

    let read = crate::runtime_tools::execute_runtime_mcp_read_resource(
        &config_path,
        None,
        "team-tools",
        "axon://team-tools/schema",
    )
    .expect("模型侧 read_mcp_resource 应可读取 MCP resource");
    assert!(read.summary.contains("team schema v1"));
    assert_eq!(read.metadata["rawPayloadExposed"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_builtin_mcp_resource_tools_keep_healthy_servers_when_one_server_fails() {
    let root = super::test_root("runtime-mcp-resources-partial");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_runtime_exec_mcp_config(&root, "team-tools");
    super::seed_mcp_server(&root, "broken-tools");
    super::seed_malformed_resources_mcp_config(&root, "broken-tools");
    let config_path = root.join("config.toml");

    let list =
        crate::runtime_tools::execute_runtime_mcp_list_resources(&config_path, None, None, None)
            .expect("全量 list_mcp_resources 应隔离单个异常 MCP server");

    assert!(list.summary.contains("axon://team-tools/schema"));
    assert_eq!(list.metadata["mcpResourceServersQueried"], 2);
    assert_eq!(list.metadata["mcpResourceServersFailed"], 1);
    assert_eq!(
        list.metadata["mcpResourceFailures"][0]["server"],
        "broken-tools"
    );
    assert_eq!(
        list.metadata["mcpResourceFailures"][0]["operation"],
        "resources/list"
    );
    let failure_message = list.metadata["mcpResourceFailures"][0]["message"]
        .as_str()
        .expect("失败 metadata 应包含错误消息");
    assert!(failure_message.contains("MCP resources/list 响应缺少 resources"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_builtin_mcp_resource_tools_report_named_server_failure() {
    let root = super::test_root("runtime-mcp-resources-named-failure");
    super::seed_mcp_server(&root, "broken-tools");
    super::seed_malformed_resources_mcp_config(&root, "broken-tools");
    let config_path = root.join("config.toml");

    let error = crate::runtime_tools::execute_runtime_mcp_list_resources(
        &config_path,
        None,
        Some("broken-tools"),
        None,
    )
    .expect_err("指定异常 MCP server 时应返回明确错误");

    assert!(error.contains("server=broken-tools"));
    assert!(error.contains("resources/list"));
    assert!(error.contains("MCP resources/list 响应缺少 resources"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_builtin_mcp_resource_cursor_requires_named_server() {
    let root = super::test_root("runtime-mcp-resources-cursor-server");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_runtime_exec_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");

    let error = crate::runtime_tools::execute_runtime_mcp_list_resources(
        &config_path,
        None,
        None,
        Some("schema-page-2"),
    )
    .expect_err("未指定 server 时不应接受 resources cursor");

    assert!(error.contains("cursor 只能在指定 MCP server 时使用"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_tool_registry_exposes_workspace_skill_tool_for_turn_time_use() {
    let root = super::test_root("runtime-skill-registry");
    let workspace = root.join("workspace");
    super::seed_workspace_skill(&workspace);
    let config_path = root.join("home").join("config.toml");

    let registry =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, Some(&workspace))
            .registry;
    let skills_snapshot =
        crate::runtime_tools::load_runtime_skills_snapshot(&config_path, Some(&workspace))
            .expect("skill snapshot 应可加载");
    let skills_context = skills_snapshot.render_available();

    assert!(
        registry
            .model_tools()
            .iter()
            .any(|tool| tool.qualified_name == "builtin.invoke_skill")
    );
    assert!(skills_context.contains("release-notes"));
    assert!(skills_context.contains("skill://workspace/release-notes"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn session_registry_exposes_mcp_schema_in_provider_payload() {
    let root = super::test_root("session-registry");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_handshake_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");

    let registry = crate::runtime_tools::build_execution_tool_registry_for_session(
        Some(&config_path),
        Some(&root),
        "session-1",
    );

    assert!(
        registry
            .model_tools()
            .iter()
            .any(|tool| tool.qualified_name == "mcp__team_tools__query_db")
    );
    assert!(
        registry
            .model_tools_for_provider()
            .iter()
            .any(|tool| tool.qualified_name == "mcp__team_tools__query_db")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_invoke_skill_returns_workspace_skill_summary() {
    let root = super::test_root("runtime-skill-exec");
    let workspace = root.join("workspace");
    super::seed_workspace_skill(&workspace);
    let config_path = root.join("home").join("config.toml");

    let summary = crate::runtime_tools::execute_runtime_skill_invocation(
        &config_path,
        Some(&workspace),
        &axon_agent_runtime::InvokeSkillInput {
            skill_name: "release-notes".to_owned(),
        },
    )
    .expect("Skill 工具应可返回 Skill 内容");

    assert!(summary.contains("[Selected Skill]"));
    assert!(summary.contains("source=skill://workspace/release-notes"));
    assert!(summary.contains("release notes skill"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_builtin_mcp_resource_tools_accept_cursor_argument() {
    let root = super::test_root("runtime-mcp-resources-cursor");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_runtime_exec_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");
    let context = tool_context(&root, Some(config_path));
    let tool_call = axon_agent_runtime::ProviderToolCall::new(
        "call-list-resources-cursor",
        "builtin.list_mcp_resources",
        r#"{"server":"team-tools","cursor":"page-2"}"#,
    );

    let receipt = crate::tool_orchestrator_runtime_capabilities::execute_list_mcp_resources(
        &tool_call, &context,
    );

    assert_eq!(receipt.status, "completed");
    let metadata = receipt.metadata.expect("工具 receipt 应包含 metadata");
    assert_eq!(metadata["cursor"], "page-2");
    assert_eq!(metadata["rawPayloadExposed"], false);
    let _ = fs::remove_dir_all(root);
}

fn tool_context(
    cwd: &Path,
    config_path: Option<PathBuf>,
) -> crate::tool_orchestrator::ToolOrchestratorContext {
    let execution_tools = Arc::new(std::sync::RwLock::new(
        crate::runtime_tools::build_execution_tool_registry_for_session(
            config_path.as_deref(),
            Some(cwd),
            "test-thread",
        ),
    ));
    crate::tool_orchestrator::ToolOrchestratorContext {
        work_contract: std::sync::Arc::new(std::sync::Mutex::new(axon_agent_runtime::WorkContractRuntime::default())),
        config_path,
        cwd: cwd.to_path_buf(),
        session_id: SessionId(1),
        thread_session_id: "test-thread".to_owned(),
        cell_id: CellId(1),
        model_messages: Vec::new(),
        execution_tools,
        memory_write_runtime: Arc::new(crate::run_commands::AgentMemoryWriteRuntime::default()),
        timeout: Duration::from_secs(15),
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: ModelProfileId(0),
        permission_snapshot_id: PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: Arc::new(Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: Arc::new(
            crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
        ),
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        user_question_port: None,
    }
}

#[test]
fn runtime_mcp_tool_executes_through_local_server_bridge() {
    let root = super::test_root("runtime-exec");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_runtime_exec_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");
    let tool_call = axon_agent_runtime::ProviderToolCall::new(
        "call-mcp-1",
        "mcp__team_tools__query_db",
        r#"{"query":"ORBIT-9"}"#,
    );
    let execution_registry =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;

    let output = crate::runtime_tools::execute_runtime_mcp_tool_call(
        &config_path,
        None,
        &execution_registry,
        &tool_call,
    )
    .expect("MCP tool 应可通过本机 server bridge 执行");

    assert!(!output.summary.trim().is_empty());
    assert!(output.summary.contains("MCP_RESULT"));
    assert_eq!(output.metadata["transport"], "stdio");
    assert_eq!(
        output.metadata["result"]["structuredContentKeys"][0],
        "files"
    );
    assert_eq!(output.metadata["files"][0]["path"], "src/lib.rs");
    assert_eq!(output.metadata["files"][0]["operation"], "update");
    assert!(
        output.metadata["files"][0]["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("-old") && diff.contains("+new"))
    );
    assert_eq!(output.metadata["rawPayloadExposed"], false);
    assert!(output.filesystem_write_performed);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn runtime_mcp_tools_survive_model_name_collision() {
    let root = super::test_root("runtime-collision");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_collision_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");

    let registry =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;
    let mut mcp_tools = registry
        .model_tools()
        .into_iter()
        .filter(|tool| {
            tool.qualified_name
                .starts_with("mcp__team_tools__query_db_")
        })
        .collect::<Vec<_>>();
    mcp_tools.sort_by(|left, right| left.qualified_name.cmp(&right.qualified_name));

    assert_eq!(mcp_tools.len(), 2);
    assert!(mcp_tools.iter().all(|tool| tool.qualified_name.len() <= 64));
    let _ = fs::remove_dir_all(root);
}

/// 调用期失败只允许冷却当次工具：不得写 server 级熔断，不得摘除工具，也不能连带同 server 的其他工具。
#[test]
fn runtime_mcp_call_failure_cools_only_that_tool_and_keeps_registry() {
    let root = super::test_root("runtime-call-cooldown");
    // 握手脚本在 tools/list 之后立即退出：tools/call 必然失败，且失败原因不是 server 业务拒绝。
    super::seed_handshake_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");
    let execution_registry =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;
    let call = |name: &str| {
        axon_agent_runtime::ProviderToolCall::new(
            "call-mcp-cooldown",
            name,
            r#"{"query":"ORBIT-9"}"#,
        )
    };

    let first = crate::runtime_tools::execute_runtime_mcp_tool_call(
        &config_path,
        None,
        &execution_registry,
        &call("mcp__team_tools__query_db"),
    )
    .expect_err("server 已退出时 tools/call 必须失败");
    assert_eq!(
        first.kind(),
        RuntimeMcpToolCallFailureKind::Unavailable,
        "{}",
        first.message()
    );
    assert!(
        !first.message().contains("返回 error"),
        "会话断开属于传输类失败，不得误判为 server 业务拒绝：{}",
        first.message()
    );

    let cooled = crate::runtime_tools::execute_runtime_mcp_tool_call(
        &config_path,
        None,
        &execution_registry,
        &call("mcp__team_tools__query_db"),
    )
    .expect_err("冷却窗口内必须快速失败");
    assert_eq!(
        cooled.kind(),
        RuntimeMcpToolCallFailureKind::ToolCooling,
        "失败工具必须进入短期冷却：{}",
        cooled.message()
    );
    assert!(cooled.message().contains("短期冷却"));

    let sibling = crate::runtime_tools::execute_runtime_mcp_tool_call(
        &config_path,
        None,
        &execution_registry,
        &call("mcp__team_tools__explain_db"),
    )
    .expect_err("同 server 的另一个工具仍会真实调用并失败");
    assert_eq!(
        sibling.kind(),
        RuntimeMcpToolCallFailureKind::Unavailable,
        "同 server 的其他工具不得被连带冷却：{}",
        sibling.message()
    );

    let rebuilt =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;
    assert!(
        rebuilt
            .model_tools()
            .iter()
            .any(|tool| tool.qualified_name == "mcp__team_tools__query_db"),
        "调用期失败不得把该 server 的工具从注册表摘除"
    );
    let _ = fs::remove_dir_all(root);
}

/// `toolTimeoutMs` 非法属于本地配置错误：fast-fail，且第二次调用仍是配置错误而不是冷却。
#[test]
fn runtime_mcp_invalid_tool_timeout_fails_fast_without_negative_cache() {
    let root = super::test_root("runtime-config-error");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_runtime_exec_mcp_config_with_tool_timeout(&root, "team-tools", 0);
    let config_path = root.join("config.toml");
    let execution_registry =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;
    let tool_call = axon_agent_runtime::ProviderToolCall::new(
        "call-mcp-config",
        "mcp__team_tools__query_db",
        r#"{"query":"ORBIT-9"}"#,
    );

    let first = crate::runtime_tools::execute_runtime_mcp_tool_call(
        &config_path,
        None,
        &execution_registry,
        &tool_call,
    )
    .expect_err("toolTimeoutMs=0 必须 fail-fast");
    assert_eq!(first.kind(), RuntimeMcpToolCallFailureKind::InvalidConfig);
    assert!(first.message().contains("toolTimeoutMs"));

    let second = crate::runtime_tools::execute_runtime_mcp_tool_call(
        &config_path,
        None,
        &execution_registry,
        &tool_call,
    )
    .expect_err("配置错误重试仍是配置错误");
    assert_eq!(
        second.kind(),
        RuntimeMcpToolCallFailureKind::InvalidConfig,
        "配置错误不得留下冷却记录：{}",
        second.message()
    );

    let rebuilt =
        crate::runtime_tools::build_runtime_tool_registry_snapshot(&config_path, None).registry;
    assert!(
        rebuilt
            .model_tools()
            .iter()
            .any(|tool| tool.qualified_name == "mcp__team_tools__query_db"),
        "配置错误不得摘除工具"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn startup_snapshot_cache_reprobes_failed_mcp_after_ttl() {
    let root = super::test_root("snapshot-mcp-retry");
    // 只有配置、没有握手脚本：该 server 在快照构建中必然探测失败。
    super::seed_mcp_server(&root, "broken-tools");
    let config_path = root.join("config.toml");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("工作区应可创建");
    let cache = crate::run_commands::StartupSnapshotCache::default();

    let first = cache
        .acquire(&config_path, &workspace)
        .expect("首次应构建快照");
    assert!(
        first.has_failed_mcp_servers(),
        "探测失败的 MCP server 必须记入快照时间线"
    );

    cache.expire_for_tests(&config_path, &workspace);
    let second = cache
        .acquire(&config_path, &workspace)
        .expect("TTL 到期后应重建");

    assert!(
        !std::sync::Arc::ptr_eq(&first, &second),
        "存在失败 MCP 时 TTL 到期必须重跑探测，而不是只续期"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn startup_snapshot_cache_renews_when_no_mcp_failure() {
    let root = super::test_root("snapshot-mcp-renew");
    super::seed_mcp_server(&root, "team-tools");
    super::seed_handshake_mcp_config(&root, "team-tools");
    let config_path = root.join("config.toml");
    let workspace = root.join("workspace");
    fs::create_dir_all(&workspace).expect("工作区应可创建");
    let cache = crate::run_commands::StartupSnapshotCache::default();

    let first = cache
        .acquire(&config_path, &workspace)
        .expect("首次应构建快照");
    assert!(!first.has_failed_mcp_servers(), "健康 server 不应记入失败");

    cache.expire_for_tests(&config_path, &workspace);
    let second = cache
        .acquire(&config_path, &workspace)
        .expect("TTL 到期后续期");

    assert!(
        std::sync::Arc::ptr_eq(&first, &second),
        "全部 MCP 健康时 TTL 到期只续期，不得重复探测"
    );
    let _ = fs::remove_dir_all(root);
}
