use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use axon_agent_runtime::{
    AgentModelMessage, AgentModelPayload, AgentTurnEvent, AgentTurnEventKind, ProviderToolCall,
    SEMANTIC_SALIENCE_SCHEMA_VERSION, SelfIterationRuntime, SemanticMemoryCandidateType,
    SemanticSalienceAssessment, SemanticSalienceSignals, TurnOutcomeEnvelope,
};
use axon_config::{AxonConfig, ChannelBalancerConfig, ModelConfig, ProviderConfig};
use axon_domain::{SessionId, TurnId};
use axon_memory::MemoryRuntimeQualityStore;
use axon_memory_read::init_memory_orchestrator;
use axon_protocol::wire::WireEventKind;

use super::{
    RunRequest,
    run_execution::{RunJsonEvent, RunResponse},
    run_json_wire_events::CompletedRunWireEventsInput,
    run_post_run_governance_worker::run_governance_worker_with_request,
    run_progress::RunProgressSink,
    run_self_iteration_subagent::self_iteration_input,
    run_subagent_executor::{
        RunSubagentExecutionInput, RunSubagentExecutionOutput, RunSubagentExecutionStatus,
        RunSubagentExecutor,
    },
    run_tool_loop::RunToolReceipt,
    run_turn_outcome::build_turn_outcome,
    run_turn_spec::TurnSpec,
    run_turn_terminal::complete_turn,
};

#[test]
fn ordinary_completed_turn_does_not_delegate_deep_reflection() {
    let root = test_root("trivial-self-iteration");
    let request = request_fixture(root.clone(), root.join("config.toml"));
    let response = response_with_receipts(&[]);

    assert!(self_iteration_labels(&request, &response).is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn selected_project_skill_without_execution_evidence_is_not_reflected() {
    let root = test_root("short-skill-reflection");
    let mut request = request_fixture(root.clone(), root.join("config.toml"));
    request.selected_skill_name = Some("release".to_owned());
    request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
    let response = response_with_receipts(&[]);

    assert!(self_iteration_labels(&request, &response).is_empty());

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completed_run_governs_reflection_off_main_path() {
    let root = test_root("low-risk-post-run");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let skill_file = root
        .join(".axon")
        .join("skills")
        .join("release")
        .join("SKILL.md");
    std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
    std::fs::write(
        &skill_file,
        "---\nname: release\ndescription: 验证发布流程\n---\n\n# Release\n",
    )
    .unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut request = request_fixture_with_executor(
        root.clone(),
        config_path.clone(),
        Arc::new(CapturingSelfIterationExecutor {
            captured: captured.clone(),
        }),
    );
    request.selected_skill_name = Some("release".to_owned());
    request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
    let mut response = response_with_receipts(&[
        receipt(
            "call-skill",
            "builtin.invoke_skill",
            "completed",
            "Skill instructions loaded",
            false,
        ),
        receipt(
            "call-edit",
            "builtin.edit",
            "completed",
            "patched files",
            true,
        ),
        receipt(
            "call-test",
            "builtin.shell_command",
            "completed",
            "validation completed",
            false,
        ),
    ]);
    response.turn_events.extend(provider_turn_events(7));
    let mut events = Vec::new();
    let mut collect = |event| events.push(event);
    let sink = RunProgressSink::with_session_id(&mut collect, "test-session".to_owned());

    let completed = complete_turn(&TurnSpec::primary(), &request, response);
    drop(sink);

    let receipt = completed
        .post_run_governance
        .as_ref()
        .expect("完成后必须启动 post-run governance");
    assert_eq!(receipt.status, "started");
    let status_path = receipt.status_path.as_ref().expect("必须返回 statusPath");
    assert_eq!(
        run_governance_worker_with_request(&config_path, &AxonConfig::default(), &request),
        Ok(1)
    );
    let json_event = RunJsonEvent::completed(CompletedRunWireEventsInput {
        request: &request,
        response: &completed.response,
        session_title: completed.session_title.as_deref(),
        memory_usefulness: completed.memory_usefulness.as_ref(),
        memory_autonomy: completed.memory_autonomy.as_ref(),
        post_run_governance: completed.post_run_governance.as_ref(),
        recorded_at_ms: super::run_execution::wire_recorded_at_ms(),
        total_elapsed_ms: 0,
    });
    let json_value = serde_json::to_value(json_event).unwrap();
    assert_eq!(json_value["status"], "success");
    let wire_events = json_value["wireEvents"]
        .as_array()
        .expect("必须输出 wireEvents");
    assert_eq!(
        wire_events
            .iter()
            .filter(|event| event["kind"]["type"] == "toolResult")
            .count(),
        3
    );
    let governance = wire_events
        .iter()
        .find(|event| event["kind"]["type"] == "governanceStatus")
        .expect("必须输出 governanceStatus wire event");
    assert_eq!(governance["kind"]["payload"]["status"], "started");
    assert_eq!(governance["kind"]["payload"]["statusPath"], *status_path);

    assert_eq!(
        governance_job_status(std::path::Path::new(status_path), &receipt.job_id),
        "completed"
    );

    // 终态记账必须给这一轮留下指标：缺行时界面上的耗时与用量没有基线。
    let store = crate::run_commands::run_agent_store::shared_runtime_store(&config_path)
        .expect("共享 runtime 库必须可解析");
    let session_id =
        axon_domain::DaemonSessionId::new(request.session_id.clone()).expect("合法会话标识");
    let replayed = axon_session::EventLogStore::replay_events(&store, &session_id, 0, 64)
        .expect("必须能读回本轮事件");
    assert!(
        replayed
            .iter()
            .any(|event| event.event_type == "turn_metrics"),
        "完成一轮后必须落下 turn_metrics，实际事件类型 {:?}",
        replayed
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>()
    );

    let kinds = events
        .iter()
        .map(|event| event.event.kind.clone())
        .collect::<Vec<_>>();
    assert!(
        kinds.iter().all(|kind| !matches!(
            kind,
            WireEventKind::SkillStatus(_) | WireEventKind::ReviewStatus(_)
        )),
        "反思链路不应产生主路径技能状态或人审状态事件"
    );
    assert!(captured_labels_contain(&captured, &["memory_update"]));
    let assessor_input = captured_input_ending_with(&captured, "semantic_salience")
        .expect("post-run 应先执行 semantic salience assessor");
    assert!(!assessor_input.tools_enabled);
    assert!(assessor_input.structured_output.is_some());
    assert!(
        assessor_input
            .system_prompt_override
            .as_deref()
            .is_some_and(|prompt| prompt.contains("只输出符合 schema 的 JSON"))
    );
    let orchestrator = init_memory_orchestrator(&config_path, &AxonConfig::default()).unwrap();
    let quality = MemoryRuntimeQualityStore::open(orchestrator.config())
        .unwrap()
        .report()
        .unwrap();
    assert_eq!(quality.observation_count, 1);
    assert_eq!(quality.semantic_assessments, 1);
    assert_eq!(quality.reflection_attempts, 1);
    // 该 executor 不写记忆，反思产出恒为 0；与线上实测完全一致。
    assert_eq!(quality.reflection_yields, 0);
    assert_eq!(quality.reflection_yield_basis_points, 0);
    assert_eq!(quality.false_memory_rejections, 0);
    assert_no_running_subagent_sessions(&request);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completed_run_keeps_high_risk_selected_skill_untouched() {
    let root = test_root("high-risk-post-run");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let skill_file = root
        .join(".axon")
        .join("skills")
        .join("release")
        .join("SKILL.md");
    std::fs::create_dir_all(skill_file.parent().unwrap()).unwrap();
    let original_skill = "---\nname: release\ndescription: 验证发布流程\n---\n\n# Release\n";
    std::fs::write(&skill_file, original_skill).unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let mut request = request_fixture_with_executor(
        root.clone(),
        config_path.clone(),
        Arc::new(CapturingSelfIterationExecutor {
            captured: captured.clone(),
        }),
    );
    request.selected_skill_name = Some("release".to_owned());
    request.selected_skill_source_ref = Some("skill://axon-project-skills/release".to_owned());
    let response = response_with_receipts(&[
        receipt(
            "call-skill",
            "builtin.invoke_skill",
            "completed",
            "Skill instructions loaded",
            false,
        ),
        receipt(
            "call-edit",
            "builtin.edit",
            "completed",
            "authorization: bearer secret",
            true,
        ),
        receipt(
            "call-test",
            "builtin.shell_command",
            "completed",
            "validation completed",
            false,
        ),
    ]);
    let mut events = Vec::new();
    let mut collect = |event| events.push(event);
    let sink = RunProgressSink::with_session_id(&mut collect, "test-session".to_owned());

    let completed = complete_turn(&TurnSpec::primary(), &request, response);
    drop(sink);

    let receipt = completed
        .post_run_governance
        .as_ref()
        .expect("完成后必须启动 post-run governance");
    assert_eq!(receipt.status, "started");
    assert!(receipt.status_path.is_some(), "必须返回 statusPath");
    assert_eq!(
        run_governance_worker_with_request(&config_path, &AxonConfig::default(), &request),
        Ok(1)
    );
    assert_eq!(
        std::fs::read_to_string(&skill_file).unwrap(),
        original_skill
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event.kind, WireEventKind::SkillStatus(_))),
        "高风险静默时不应向 TUI 回显技能状态"
    );
    assert!(captured_labels_contain(&captured, &["memory_update"]));
    assert_no_running_subagent_sessions(&request);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn durable_governance_requeues_failed_subagent_execution() {
    let root = test_root("failed-subagent-retry");
    let config_path = root.join("runtime").join("config.toml");
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let request = request_fixture_with_executor(
        root.clone(),
        config_path.clone(),
        Arc::new(AssessorOnlyExecutor),
    );
    let mut response = response_with_receipts(&[
        receipt("call-1", "builtin.edit", "completed", "patched files", true),
        receipt(
            "call-2",
            "builtin.shell_command",
            "completed",
            "cargo check passed",
            false,
        ),
        receipt(
            "call-3",
            "builtin.shell_command",
            "completed",
            "cargo test passed",
            false,
        ),
    ]);
    response.turn_events.extend(provider_turn_events(7));

    let completed = complete_turn(&TurnSpec::primary(), &request, response);
    let receipt = completed
        .post_run_governance
        .as_ref()
        .expect("完成后必须启动 post-run governance");
    let status_path = receipt.status_path.as_ref().expect("必须返回 statusPath");

    assert_eq!(
        run_governance_worker_with_request(&config_path, &AxonConfig::default(), &request),
        Ok(0)
    );
    assert_eq!(
        governance_job_status(std::path::Path::new(status_path), &receipt.job_id),
        "queued"
    );
    assert!(
        governance_job_error(std::path::Path::new(status_path), &receipt.job_id)
            .contains("1 个子任务未完成")
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 构造一条形态真实的一轮响应：每个回执都有自己的调用、生命周期事件与唯一 part。
///
/// thread 事件流是轮次事实的唯一来源，`mirror_session_to_thread` 只投影「有调用、
/// 有 started 事件、part 互不相同」的回执；夹具必须跟生产形态一致，否则治理读到
/// 的轮次就不是这一轮。
fn response_with_receipts(receipts: &[RunToolReceipt]) -> RunResponse {
    let started = receipts
        .iter()
        .enumerate()
        .map(|(index, _)| {
            AgentTurnEvent::new(
                TurnId(u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1)),
                AgentTurnEventKind::ToolExecutionStarted,
                "tool execution started",
            )
        })
        .collect::<Vec<_>>();
    let tool_receipts = receipts
        .iter()
        .enumerate()
        .map(|(index, receipt)| RunToolReceipt {
            tool_call_id: u64::try_from(index).unwrap_or(u64::MAX).saturating_add(1),
            part_id: started[index].part_id.clone(),
            ..receipt.clone()
        })
        .collect::<Vec<_>>();
    let tool_calls = tool_receipts
        .iter()
        .map(|receipt| {
            ProviderToolCall::new(
                receipt.provider_call_id.clone(),
                receipt.tool_name.clone(),
                tool_call_input(&receipt.tool_name),
            )
        })
        .collect();
    RunResponse {
        content: "done".to_owned(),
        assistant_part_id: "assistant-part:post-run-reflection".to_owned(),
        provider: "fixture".to_owned(),
        model: "fixture-chat".to_owned(),
        token_usage: None,
        http_status: 200,
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        tool_receipts,
        tool_calls,
        turn_events: started,
        max_tokens_truncated: false,
    }
}

/// 与工具名一致的调用输入：参数指纹与证据归因都按真实调用形态取值。
fn tool_call_input(tool_name: &str) -> String {
    match tool_name {
        "builtin.invoke_skill" => r#"{"skill_name":"release"}"#.to_owned(),
        "builtin.edit" => r#"{"path":"src/lib.rs"}"#.to_owned(),
        _ => r#"{"command":"cargo test"}"#.to_owned(),
    }
}

fn receipt(
    provider_call_id: &str,
    tool_name: &str,
    status: &str,
    summary: &str,
    wrote: bool,
) -> RunToolReceipt {
    RunToolReceipt {
        tool_call_id: 0,
        part_id: "tool-part:0".to_owned(),
        provider_call_id: provider_call_id.to_owned(),
        tool_name: tool_name.to_owned(),
        status: status.to_owned(),
        summary: summary.to_owned(),
        metadata: None,
        error_type: None,
        remote_trigger_allowed: false,
        filesystem_write_performed: wrote,
        output_ref: None,
        duration_ms: None,
    }
}

#[path = "run_post_run_reflection_fixtures.rs"]
mod fixtures;
use fixtures::*;
