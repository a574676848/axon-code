//! 两个测试模块共用的上下文构造与拒绝文案判定。
//!
//! 单独成文件：`orchestrator_builtin_tests` 与 `orchestrator_intent_tests` 都要用，
//! 任一测试文件各自持有一份会把单文件推到行数上限之上。

use super::*;

pub(super) fn make_test_context(
    config_path: std::path::PathBuf,
    session_id: &str,
) -> ToolOrchestratorContext {
    ToolOrchestratorContext {
        config_path: Some(config_path),
        cwd: std::env::current_dir().unwrap_or_default(),
        session_id: axon_domain::SessionId(1),
        thread_session_id: session_id.to_owned(),
        cell_id: axon_domain::CellId(1),
        model_messages: Vec::new(),
        execution_tools: std::sync::Arc::new(std::sync::RwLock::new(
            axon_agent_runtime::ToolRegistry::new(),
        )),
        memory_write_runtime: std::sync::Arc::new(
            crate::run_commands::AgentMemoryWriteRuntime::default(),
        ),
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        timeout: std::time::Duration::from_secs(15),
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        run_cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
        supports_vision: true,
        permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: std::sync::Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor: std::sync::Arc::new(
            crate::run_commands::run_subagent_executor::NoopRunSubagentExecutor,
        ),
        subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        user_question_port: None,
    }
}

/// 执行前参数被拒的稳定文案标记：出现任一即说明工具在执行前拒绝了这次调用。
pub(super) fn contains_input_rejection(summary: &str) -> bool {
    [
        "工具入参",
        "不符合 schema",
        "包含不支持的内容",
        "无法读取",
        "unknown field",
        "missing field",
    ]
    .iter()
    .any(|marker| summary.contains(marker))
}
