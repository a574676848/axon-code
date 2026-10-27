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
    RunRequest, RunResponse, finish_completed_run,
    run_execution::RunJsonEvent,
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
fn completed_run_applies_agent_reflection_to_selected_skill_off_main_path() {
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
    response.turn_events = provider_turn_events(7);
    let mut events = Vec::new();
    let mut collect = |event| events.push(event);
    let sink = RunProgressSink::with_session_id(&mut collect, "test-session".to_owned());

    let completed = finish_completed_run(&config_path, &request, response)
        .expect("post-run 完成链路不应被 session/thread 副作用打断");
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
    assert!(completed.skill_evolution.is_none());
    let json_event = RunJsonEvent::completed(CompletedRunWireEventsInput {
        request: &request,
        response: &completed.response,
        session_title: completed.session_title.as_deref(),
        memory_usefulness: completed.memory_usefulness.as_ref(),
        memory_autonomy: completed.memory_autonomy.as_ref(),
        skill_evolution: completed.skill_evolution.as_ref(),
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

    let content = std::fs::read_to_string(&skill_file).expect("worker 应写入低风险 Skill");
    assert_eq!(
        governance_job_status(std::path::Path::new(status_path), &receipt.job_id),
        "completed"
    );
    assert!(content.contains("## 已验证经验"));
    assert!(content.contains("子代理自闭环完成"));

    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event.kind, WireEventKind::SkillStatus(_))),
        "skill evolution 不应恢复主路径 skill 状态"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event.kind, WireEventKind::ReviewStatus(_))),
        "skill evolution 不应恢复人审状态"
    );
    assert!(captured_labels_contain(
        &captured,
        &["memory_update", "skill_reflection"]
    ));
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
    assert_eq!(quality.reflection_attempts, 2);
    assert_eq!(quality.reflection_yields, 1);
    assert_eq!(quality.reflection_yield_basis_points, 5_000);
    assert_eq!(quality.false_memory_rejections, 0);
    assert_no_running_subagent_sessions(&request);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn completed_run_records_high_risk_selected_skill_rejection() {
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

    let completed = finish_completed_run(&config_path, &request, response)
        .expect("高风险静默 trace 不应打断 post-run 完成链路");
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
    assert!(completed.skill_evolution.is_none());
    assert_eq!(
        std::fs::read_to_string(&skill_file).unwrap(),
        original_skill
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.event.kind, WireEventKind::SkillStatus(_))),
        "高风险静默时不应向 TUI 回显 skill evolution"
    );
    let state_path = config_path
        .parent()
        .unwrap()
        .join("skill-evolution")
        .join("state.json");
    let state = std::fs::read_to_string(&state_path).expect("worker 应写入拒绝状态");
    assert_eq!(
        governance_job_status(std::path::Path::new(status_path), &receipt.job_id),
        "completed"
    );
    assert!(state.contains("\"kind\": \"improvement\""));
    assert!(state.contains("\"risk\": \"high\""));
    assert!(state.contains("\"stage\": \"rejected\""));
    assert!(captured_labels_contain(
        &captured,
        &["memory_update", "skill_reflection"]
    ));
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
    response.turn_events = provider_turn_events(7);

    let completed = finish_completed_run(&config_path, &request, response)
        .expect("完成链路应成功持久化 governance job");
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

fn response_with_receipts(receipts: &[RunToolReceipt]) -> RunResponse {
    let tool_calls = receipts
        .iter()
        .filter(|receipt| receipt.tool_name == "builtin.invoke_skill")
        .map(|receipt| {
            ProviderToolCall::new(
                receipt.provider_call_id.clone(),
                receipt.tool_name.clone(),
                r#"{"skill_name":"release"}"#,
            )
        })
        .collect();
    RunResponse {
        content: "done".to_owned(),
        assistant_part_id: "assistant-part:skill-evolution-post-run".to_owned(),
        provider: "fixture".to_owned(),
        model: "fixture-chat".to_owned(),
        token_usage: None,
        http_status: 200,
        channel_attempts: Vec::new(),
        reasoning_messages: Vec::new(),
        tool_receipts: receipts.to_vec(),
        tool_calls,
        turn_events: Vec::new(),
        max_tokens_truncated: false,
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

fn request_fixture(root: std::path::PathBuf, config_path: std::path::PathBuf) -> RunRequest {
    request_fixture_with_executor(
        root,
        config_path,
        Arc::new(super::run_subagent_executor::NoopRunSubagentExecutor),
    )
}

fn request_fixture_with_executor(
    root: std::path::PathBuf,
    config_path: std::path::PathBuf,
    subagent_executor: Arc<dyn RunSubagentExecutor>,
) -> RunRequest {
    let session_id = "session-1".to_owned();
    let (startup, execution_tools) =
        super::run_startup_snapshot::test_startup_runtime(&config_path, &root, &session_id);
    RunRequest {
        user_question_port: None,
        client_channel: axon_client_identity::ClientChannel::Cli,
        config_path,
        session_id: session_id.clone(),
        invocation_id: format!("test-invocation:{session_id}"),
        resumed_from: None,
        parent_session_id: None,
        cwd: root,
        prompt: "生成可复用 skill".to_owned(),
        quoted_message: None,
        structured_output: None,
        selected_skill_name: None,
        selected_skill_source_ref: None,
        model_payload: AgentModelPayload {
            messages: vec![AgentModelMessage::text("user", "生成可复用 skill")],
            model_tools: Vec::new(),
            context_section_count: 0,
            agent_pane_included: false,
            injected_context_messages: Vec::new(),
        },
        persona_warnings: Vec::new(),
        startup,
        execution_tools,
        provider: ProviderConfig::default(),
        model: ModelConfig::default(),
        reasoning_effort: None,
        channel_chain: Vec::new(),
        routing_key: "fixture".to_owned(),
        balancer: ChannelBalancerConfig::default(),
        timeout: Duration::from_secs(30),
        memory_recall: None,
        goal_disclosure_policy: axon_domain::DisclosurePolicy::PrivateOnly,
        goal_transport_context: None,
        cancel_token: None,
        model_profile_id: axon_domain::ModelProfileId(0),
        permission_snapshot_id: axon_domain::PermissionSnapshotId(0),
        current_agent_path: axon_agent_runtime::AgentPath::root(),
        agent_control: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::AgentControlRuntime::default(),
        )),
        agent_graph_store: std::sync::Arc::new(axon_agent_runtime::LocalAgentGraphStore::default()),
        subagent_sessions: std::sync::Arc::new(std::sync::Mutex::new(
            axon_agent_runtime::SubagentSessionRuntime::default(),
        )),
        subagent_executor,
        subagent_cancel_tokens: std::sync::Arc::new(std::sync::Mutex::new(
            std::collections::HashMap::new(),
        )),
        steer_inbox: None,
        workspace_files: std::sync::Arc::new(axon_agent_runtime::WorkspaceFileSession::default()),
        auto_compaction_threshold: 0.8,
        performance: None,
    }
}

#[derive(Debug)]
struct CapturingSelfIterationExecutor {
    captured: Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
}

impl RunSubagentExecutor for CapturingSelfIterationExecutor {
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        let is_assessor = input.task_path.name().ends_with("semantic_salience");
        self.captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(input);
        Some(RunSubagentExecutionOutput {
            status: RunSubagentExecutionStatus::Completed,
            summary: if is_assessor {
                semantic_assessment_json()
            } else {
                "子代理自闭环完成".to_owned()
            },
            memory_write_count: 0,
            token_usage: None,
            model: None,
            budget_exhausted: false,
        })
    }
}

#[derive(Debug)]
struct AssessorOnlyExecutor;

impl RunSubagentExecutor for AssessorOnlyExecutor {
    fn execute(&self, input: RunSubagentExecutionInput) -> Option<RunSubagentExecutionOutput> {
        input
            .task_path
            .name()
            .ends_with("semantic_salience")
            .then(|| RunSubagentExecutionOutput {
                status: RunSubagentExecutionStatus::Completed,
                summary: semantic_assessment_json(),
                memory_write_count: 0,
                token_usage: None,
                model: None,
                budget_exhausted: false,
            })
    }
}

fn semantic_assessment_json() -> String {
    serde_json::json!({
        "schemaVersion": SEMANTIC_SALIENCE_SCHEMA_VERSION,
        "shouldReflect": true,
        "candidateTypes": ["experience"],
        "signals": {
            "identityRelevance": 0,
            "stability": 8_000,
            "futureUtility": 9_000,
            "novelty": 8_000,
            "correctionStrength": 0,
            "verificationStrength": 9_000,
            "scopeConfidence": 9_000
        },
        "evidenceRefs": ["receipt:3"],
        "reason": "已验证的执行经验可跨会话复用"
    })
    .to_string()
}

fn captured_labels_contain(
    captured: &Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
    expected: &[&str],
) -> bool {
    let labels = captured_labels(captured);
    expected
        .iter()
        .all(|label| labels.iter().any(|actual| actual.ends_with(label)))
}

fn captured_labels(captured: &Arc<Mutex<Vec<RunSubagentExecutionInput>>>) -> Vec<String> {
    captured
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|input| input.task_path.name().to_owned())
        .collect()
}

fn captured_input_ending_with(
    captured: &Arc<Mutex<Vec<RunSubagentExecutionInput>>>,
    suffix: &str,
) -> Option<RunSubagentExecutionInput> {
    captured
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|input| input.task_path.name().ends_with(suffix))
        .cloned()
}

fn self_iteration_labels(request: &RunRequest, response: &RunResponse) -> Vec<String> {
    let turn_outcome = build_turn_outcome(request, response);
    let assessment = assessment_for_outcome(&turn_outcome);
    SelfIterationRuntime
        .plan_with_delegation(
            SessionId(1),
            false,
            &self_iteration_input(request, turn_outcome, Some(assessment)),
        )
        .expect("self-iteration 计划应有效")
        .delegation_plan
        .map_or_else(Vec::new, |plan| {
            plan.entries.into_iter().map(|entry| entry.label).collect()
        })
}

fn assessment_for_outcome(outcome: &TurnOutcomeEnvelope) -> SemanticSalienceAssessment {
    let evidence_ref = outcome
        .execution_evidence
        .records
        .iter()
        .rev()
        .find(|record| record.runtime_verified)
        .map(|record| record.evidence_ref.clone());
    let should_reflect = evidence_ref.is_some();
    SemanticSalienceAssessment {
        schema_version: SEMANTIC_SALIENCE_SCHEMA_VERSION,
        should_reflect,
        candidate_types: should_reflect
            .then_some(SemanticMemoryCandidateType::Experience)
            .into_iter()
            .collect(),
        signals: SemanticSalienceSignals {
            future_utility: if should_reflect { 9_000 } else { 0 },
            verification_strength: if should_reflect { 9_000 } else { 0 },
            scope_confidence: if should_reflect { 9_000 } else { 0 },
            ..SemanticSalienceSignals::default()
        },
        evidence_refs: evidence_ref.into_iter().collect(),
        reason: if should_reflect {
            "已验证的执行经验".to_owned()
        } else {
            "普通任务没有长期价值".to_owned()
        },
        user_affect: None,
        persona_drift_bp: None,
        drift_reason: None,
        style_candidate: None,
    }
}

fn provider_turn_events(count: u64) -> Vec<AgentTurnEvent> {
    (1..=count)
        .map(|turn_id| {
            AgentTurnEvent::new(
                TurnId(turn_id),
                AgentTurnEventKind::TurnStarted,
                format!("provider turn {turn_id} started"),
            )
        })
        .collect()
}

fn governance_job_status(path: &std::path::Path, job_id: &str) -> String {
    rusqlite::Connection::open(path)
        .expect("governance SQLite 应可读取")
        .query_row(
            "SELECT status FROM governance_jobs WHERE job_id = ?1",
            rusqlite::params![job_id],
            |row| row.get(0),
        )
        .expect("governance job 应存在")
}

fn governance_job_error(path: &std::path::Path, job_id: &str) -> String {
    rusqlite::Connection::open(path)
        .expect("governance SQLite 应可读取")
        .query_row(
            "SELECT last_error FROM governance_jobs WHERE job_id = ?1",
            rusqlite::params![job_id],
            |row| row.get(0),
        )
        .expect("governance job 应记录失败原因")
}

fn assert_no_running_subagent_sessions(request: &RunRequest) {
    let sessions = request
        .subagent_sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        sessions
            .sessions()
            .iter()
            .all(|handle| handle.status != axon_agent_runtime::SubagentSessionStatus::Running),
        "post-run self-iteration 子代理必须完成会话闭环"
    );
}

fn test_root(label: &str) -> std::path::PathBuf {
    static NEXT_TEST_ROOT_ID: AtomicU64 = AtomicU64::new(1);

    let root = axon_test_support::test_temp_root().join(format!(
        "axon-skill-evolution-{label}-{}-{}",
        std::process::id(),
        NEXT_TEST_ROOT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}
