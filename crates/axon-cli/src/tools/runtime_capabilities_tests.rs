use super::*;
use axon_agent_runtime::{
    AgentControlRuntime, AgentPath, LocalAgentGraphStore, SubagentSessionRuntime,
};
use axon_domain::{
    CellId, ExecutionRequestId, ModelProfileId, PermissionSnapshotId, SessionId, ToolCallId,
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[test]
fn mcp_arguments_reject_silent_optional_downgrades() {
    assert!(optional_arg(&serde_json::json!({"server": 7}), "server").is_err());
    assert!(optional_arg(&serde_json::json!({"server": "   "}), "server").is_err());
    assert!(required_arg(&serde_json::json!({}), "server").is_err());
    assert!(mcp_list_args(&serde_json::json!({"cursor": "next"})).is_err());
    assert_eq!(
        mcp_list_args(&serde_json::json!({"server": "gitnexus", "cursor": "next"})).unwrap(),
        (Some("gitnexus".to_owned()), Some("next".to_owned()))
    );
}

#[test]
fn mcp_emits_starting_and_failed_progress_around_runtime_call() {
    let workspace = temp_workspace("mcp-progress");
    let config_path = workspace.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let request = ToolExecutionRequest {
        tool_call: ProviderToolCall::new(
            "call-mcp-progress",
            "mcp__team_tools__query_db",
            r#"{"query":"ORBIT-9"}"#,
        ),
        tool_call_id: ToolCallId(1),
        part_id: "tool-part:test:mcp:progress".to_owned(),
        execution_request_id: ExecutionRequestId(1),
        kind: crate::tool_orchestrator::ToolKind::Mcp,
        concurrency_policy: axon_domain::ToolConcurrencyPolicy::Exclusive,
        execution_permission: axon_domain::ToolExecutionPermission::Allowed,
    };
    let mut events = Vec::new();
    let mut observer = |event| events.push(event);

    let receipt = execute_mcp(
        &request,
        &context(&workspace, config_path),
        Some(&mut observer),
    );

    assert_eq!(receipt.status, "failed");
    assert_eq!(
        receipt.error_type.as_deref(),
        Some(AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE)
    );
    let metadata = receipt
        .metadata
        .as_ref()
        .expect("MCP 失败应包含结构化 metadata");
    assert_eq!(metadata["server"], "team_tools");
    assert_eq!(metadata["tool"], "query_db");
    assert_eq!(metadata["degraded"], true);
    assert_eq!(metadata["rawPayloadExposed"], false);
    assert_eq!(events.len(), 2);
    let ToolRuntimeEvent::Progress(progress) = &events[0] else {
        panic!("MCP 应先发送 progress 事件");
    };
    assert_eq!(progress.phase, "starting");
    assert_eq!(progress.items_completed, Some(0));
    assert_eq!(progress.items_total, Some(1));
    assert_eq!(
        progress.current_item.as_deref(),
        Some("mcp__team_tools__query_db")
    );
    assert_eq!(progress.kind, crate::tool_orchestrator::ToolKind::Mcp);
    let ToolRuntimeEvent::Progress(progress) = &events[1] else {
        panic!("MCP 失败时应收束 progress 事件");
    };
    assert_eq!(progress.phase, "failed");
    assert_eq!(progress.items_completed, Some(1));
    assert_eq!(progress.items_total, Some(1));
    assert_eq!(progress.message.as_deref(), Some("MCP tool 调用失败"));
    let _ = fs::remove_dir_all(workspace);
}

fn context(cwd: &Path, config_path: PathBuf) -> ToolOrchestratorContext {
    let execution_tools = Arc::new(std::sync::RwLock::new(
        crate::runtime_tools::build_execution_tool_registry_for_session(
            Some(&config_path),
            Some(cwd),
            "test-thread",
        ),
    ));
    ToolOrchestratorContext {
        config_path: Some(config_path),
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
        supports_vision: true,
        permission_snapshot_id: PermissionSnapshotId(0),
        current_agent_path: AgentPath::root(),
        agent_control: Arc::new(Mutex::new(AgentControlRuntime::default())),
        agent_graph_store: Arc::new(LocalAgentGraphStore::default()),
        subagent_sessions: Arc::new(Mutex::new(SubagentSessionRuntime::default())),
        subagent_executor: Arc::new(
            crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
        ),
        subagent_cancel_tokens: Arc::new(Mutex::new(std::collections::HashMap::new())),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        user_question_port: None,
    }
}

/// 降级回执的 metadata 必须区分作用域：整台 server 熔断、单工具冷却，以及非降级的确定性失败。
#[test]
fn mcp_failure_receipts_distinguish_failure_scope() {
    let tool_call = ProviderToolCall::new(
        "call-mcp-scope",
        "mcp__team_tools__query_db",
        r#"{"query":"ORBIT-9"}"#,
    );

    let cooling = failed_mcp_tool_receipt(
        &tool_call,
        RuntimeMcpToolCallFailureKind::ToolCooling,
        "MCP tool mcp__team_tools__query_db 处于短期冷却：MCP tool 调用失败：MCP persistent 响应超时",
    );
    assert_eq!(cooling.status, "failed");
    assert_eq!(
        cooling.error_type.as_deref(),
        Some(AGENT_RUN_TOOL_ERROR_TYPE_MCP_UNAVAILABLE)
    );
    let metadata = cooling
        .metadata
        .as_ref()
        .expect("工具冷却必须携带降级 metadata");
    assert_eq!(metadata["toolCooldown"], true);
    assert_eq!(metadata["circuitOpen"], false);
    assert_eq!(metadata["degradationClass"], "optional");
    assert_eq!(metadata["rawPayloadExposed"], false);
    assert!(cooling.summary.contains("其他工具"));

    let circuit_open = failed_mcp_tool_receipt(
        &tool_call,
        RuntimeMcpToolCallFailureKind::ServerCircuitOpen,
        "MCP server team_tools 处于短期熔断：MCP tools/list 超时",
    );
    let metadata = circuit_open
        .metadata
        .as_ref()
        .expect("server 熔断必须携带降级 metadata");
    assert_eq!(metadata["circuitOpen"], true);
    assert_eq!(metadata["toolCooldown"], false);

    for (kind, reason) in [
        (
            RuntimeMcpToolCallFailureKind::InvalidConfig,
            "MCP toolTimeoutMs 必须大于 0",
        ),
        (
            RuntimeMcpToolCallFailureKind::ToolRejected,
            "MCP tools/call 返回 error",
        ),
    ] {
        let deterministic = failed_mcp_tool_receipt(&tool_call, kind, reason);
        assert_eq!(deterministic.status, "failed");
        assert_eq!(
            deterministic.error_type.as_deref(),
            Some("execution_failed")
        );
        assert!(
            deterministic.metadata.is_none(),
            "{} 是确定性失败，不得标记为可选能力降级",
            kind.as_str()
        );
    }
}

fn temp_workspace(name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = axon_test_support::test_temp_root().join(format!("axon-{name}-{suffix}"));
    fs::create_dir_all(&path).unwrap();
    path
}
