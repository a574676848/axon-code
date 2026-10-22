use axon_agent_runtime::TokenUsageTelemetry;
use axon_domain::PaneId;
use axon_protocol::wire::WireEventKind;
use std::{sync::Arc, sync::mpsc};

use crate::run_commands::run_subagent_executor::{
    RunSubagentCompletionSink, RunSubagentExecutionInput, RunSubagentExecutionOutput,
    RunSubagentExecutionStatus, RunSubagentModelIntent,
};

const TEST_SUBAGENT_RUNTIME_MS: u64 = 65_000;
const TEST_INPUT_TOKENS: u64 = 7;
const TEST_CACHE_READ_TOKENS: u64 = 14;
const TEST_CACHE_WRITE_TOKENS: u64 = 2;
const TEST_OUTPUT_TOKENS: u64 = 5;
const TEST_MODEL_NAME: &str = "gpt-5.5";

#[test]
fn subagent_terminal_lifecycle_carries_runtime_metrics() {
    let request = super::fixture_request();
    let task_path = axon_agent_runtime::AgentPath::parse("/root/review").unwrap();
    request
        .agent_control
        .lock()
        .unwrap()
        .spawn(axon_agent_runtime::AgentSpawnRequest {
            parent_path: axon_agent_runtime::AgentPath::root(),
            task_name: "review".to_owned(),
            message: "审查".to_owned(),
            nickname: None,
            role: None,
        })
        .unwrap();
    let execution_lease = request
        .agent_control
        .lock()
        .unwrap()
        .lease_execution(&task_path)
        .unwrap();
    let parent_session_id = super::fixture_parent_session_id(&request);
    let session = request
        .subagent_sessions
        .lock()
        .unwrap()
        .start(axon_agent_runtime::SubagentSessionStartInput {
            parent_session_id,
            task_path: task_path.to_string(),
            forked_from_session_id: Some(parent_session_id),
            pane_id: PaneId(1),
            cwd: request.cwd.to_string_lossy().to_string(),
            model_profile_id: request.model_profile_id,
            permission_snapshot_id: request.permission_snapshot_id,
            created_at_ms: crate::run_commands::now_ms(),
            summary: "spawned /root/review".to_owned(),
        })
        .unwrap();
    request
        .subagent_sessions
        .lock()
        .unwrap()
        .begin_execution_by_task_path(&session.task_path, execution_lease.execution_id(), "审查")
        .unwrap();
    let (lifecycle_tx, lifecycle_rx) = mpsc::channel();
    let event_bus: crate::run_commands::run_progress::RunProgressEventBus =
        Arc::new(move |event| {
            if let WireEventKind::TurnLifecycle(lifecycle) = event.event.kind {
                lifecycle_tx
                    .send(lifecycle)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });
    let lifecycle_registry =
        crate::run_commands::run_subagent_lifecycle::SubagentLifecycleRegistry::new(
            Some(event_bus),
            request.session_id.clone(),
        );
    let publisher = lifecycle_registry.begin_execution(&task_path);
    let completion = RunSubagentCompletionSink::new(
        request.agent_control.clone(),
        request.agent_graph_store.clone(),
        request.subagent_sessions.clone(),
        execution_lease,
    )
    .with_lifecycle(lifecycle_registry, publisher);
    let started_at_ms = crate::run_commands::now_ms().saturating_sub(TEST_SUBAGENT_RUNTIME_MS);
    let input = RunSubagentExecutionInput {
        task_path,
        message: "审查".to_owned(),
        structured_output: None,
        system_prompt_override: None,
        tools_enabled: true,
        fork_context: axon_agent_runtime::SubagentForkContextReport {
            messages: Vec::new(),
            original_count: 0,
            retained_count: 0,
            dropped_count: 0,
            reasons: Vec::new(),
        },
        started_at_ms,
        max_runtime_ms: None,
        model_intent: RunSubagentModelIntent::Inherit,
    };

    completion
        .complete(
            &input,
            &RunSubagentExecutionOutput {
                status: RunSubagentExecutionStatus::Completed,
                summary: "审查完成".to_owned(),
                memory_write_count: 0,
                token_usage: Some(TokenUsageTelemetry::new(
                    TEST_INPUT_TOKENS,
                    TEST_CACHE_READ_TOKENS,
                    TEST_CACHE_WRITE_TOKENS,
                    TEST_OUTPUT_TOKENS,
                    0,
                )),
                model: Some(TEST_MODEL_NAME.to_owned()),
                budget_exhausted: false,
            },
        )
        .unwrap();

    let lifecycle = lifecycle_rx.recv().unwrap();
    assert_eq!(lifecycle.started_at_ms, Some(started_at_ms));
    assert!(lifecycle.duration_ms.unwrap_or_default() >= TEST_SUBAGENT_RUNTIME_MS);
    assert_eq!(
        lifecycle.total_tokens,
        Some(
            TEST_INPUT_TOKENS
                + TEST_CACHE_READ_TOKENS
                + TEST_CACHE_WRITE_TOKENS
                + TEST_OUTPUT_TOKENS
        )
    );
    assert_eq!(lifecycle.cache_hit_rate_basis_points, Some(6_086));
    assert_eq!(lifecycle.tool_name.as_deref(), Some(TEST_MODEL_NAME));
}
